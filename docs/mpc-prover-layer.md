# MPC Prover Layer Architecture

Moving the BPF VM from client-side execution into a decentralized MPC (Multi-Party
Computation) prover layer, so that **no single party** — not even the user — sees the
full computation. Intent stays hidden, inputs stay encrypted, outputs are committed
without revealing contents.

This document extends the privacy architecture (privacy-architecture.md) with an
alternative execution model based on MPC rather than client-side ZK proving.

---

## Why Move Beyond Client-Side ZK?

The privacy architecture document describes client-side proving: the user runs the
BPF VM locally, sees all their own data, and generates a ZK proof. This works well
for single-user transactions (transfers, swaps where you know your own amounts).

But it breaks down for **multi-party scenarios**:

| Scenario | Client-Side ZK Problem |
|----------|----------------------|
| Sealed-bid auction | Who runs the matching? They'd see all bids. |
| Dark pool / private DEX | Someone has to match orders — they see intent. |
| Private lending | Liquidation engines need to see positions to check health. |
| Collaborative analytics | Multiple parties contribute data nobody should see individually. |
| Cross-user state | Programs that read multiple users' private state simultaneously. |

In these cases, the computation itself must happen on encrypted data. No single node,
no sequencer, no user should see the full picture. This is where MPC comes in.

---

## Existing Approaches

### Arcium (MPC + Secret Sharing)

Arcium's MXEs (Multi-Party eXecution Environments) are the closest to what we want.

```
How Arcium works:

  1. User encrypts inputs and secret-shares them across Arx nodes
  2. Each Arx node holds one fragment — no node sees the full data
  3. Nodes jointly compute on their fragments using MPC protocols
  4. Result is reconstructed only when all parties agree
  5. Computation is verified on-chain (Solana) via commitment

Key properties:
  - Dishonest majority: only 1 honest node needed for security
  - Cheater identification: malicious nodes are cryptographically caught + slashed
  - Parallel MXEs: multiple computations run simultaneously across different clusters
  - Custom trust: users configure which nodes, how many, what protocols
```

Arcium uses their own **Arcis** DSL (Rust-based) for writing MPC programs — not a
general-purpose VM. Their C-SPL standard brings confidential tokens to Solana.

### Partisia Blockchain (MPC Smart Contracts)

Partisia runs MPC smart contracts where inputs are secret-shared to computation nodes.
Bids in an auction, for example, are never revealed to any node — only the result
(winning bid) is published. Has a full smart contract model with encrypted inputs.

### Nillion (Blind Compute)

Nillion's "blind computer" stores and processes data while it stays encrypted.
Uses secret sharing + custom MPC protocols. Building an L2 on Ethereum. Focused on
privacy-preserving storage and computation for AI/data use cases.

### What None of Them Do

**None of these projects run a general-purpose VM (like BPF) inside MPC.** They all
use custom DSLs, restricted computation models, or application-specific protocols.

Running an arbitrary BPF program on secret-shared data is an open problem. The reason:
MPC is efficient for arithmetic (additions, multiplications over a field) but very
expensive for VM-like operations (conditional branching, dynamic memory access,
arbitrary control flow).

This is the core challenge we need to solve.

---

## The Cost of MPC Computation

To understand the architecture, you need to understand what's cheap and expensive
in MPC.

### Operation Costs (MPC on Secret-Shared Data)

| Operation | MPC Cost | Why |
|-----------|----------|-----|
| Addition (a + b) | **Free** (local) | Each node adds its shares locally. No communication. |
| Scalar multiply (a * constant) | **Free** (local) | Each node multiplies its share. No communication. |
| Multiplication (a * b) | **1 round trip** between nodes | Requires Beaver triples or OT. ~1-5ms per multiply. |
| Comparison (a > b) | **~10-20 multiplications** | Bit decomposition + comparison circuit. ~10-50ms. |
| Conditional branch | **Very expensive** | Must evaluate BOTH branches and select. Leaks nothing about which path. |
| Memory load (arr[i]) | **O(n) cost** (oblivious RAM) | Must touch ALL memory to hide which index was accessed. |
| Hash (SHA-256) | **~30,000 multiplications** | Each bit operation = multiplication. Very expensive. |
| Hash (Poseidon) | **~200 multiplications** | Designed for arithmetic circuits. Much cheaper in MPC too. |

### What This Means for a BPF VM in MPC

```
BPF instruction: ADD64 r0, r1
MPC cost:        1 addition (free — local on each node)        ✓ Fast

BPF instruction: MUL64 r0, r1
MPC cost:        1 multiplication (1 round trip)                ~ OK

BPF instruction: JEQ r0, r1, +5
MPC cost:        1 comparison (~20 multiplications)
                 + execute BOTH branches (can't reveal which)
                 + select result (~multiplications per register) ✗ Slow

BPF instruction: LDXDW r0, [r1 + offset]
MPC cost:        Oblivious RAM access — O(sqrt(N)) or O(log N)
                 multiplications where N = memory size           ✗ Very slow

Overall: A 10,000-instruction BPF program might take
         seconds to minutes in MPC (vs microseconds in plaintext).
```

**This is why nobody runs a general-purpose VM in MPC.** The overhead is prohibitive
for complex programs. But there are ways to make it practical.

---

## Architecture: Hybrid Execution Model

The key insight: **not every operation needs MPC**. Most transaction logic is
single-user (the user already knows their own data). Only specific multi-party
operations need the MPC layer.

### Three Execution Tiers

```
+------------------------------------------------------------------+
|  Tier 1: CLIENT-SIDE (Private, ZK-proven)                        |
|                                                                   |
|  User runs BPF VM locally for single-user operations:            |
|  - Private transfers (I know my own amounts)                     |
|  - Private state mutations (I know my own data)                  |
|  - Note creation/spending (my notes, my secrets)                 |
|                                                                   |
|  Trust: User sees their own data (acceptable — it's their data)  |
|  Speed: VM execution = microseconds, proof = seconds             |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
|  Tier 2: MPC PROVER LAYER (Multi-party private compute)          |
|                                                                   |
|  MPC cluster executes BPF programs on encrypted inputs:          |
|  - Order matching (sealed bids from multiple users)              |
|  - Private auctions (nobody sees losing bids)                    |
|  - Cross-user state reads (check if conditions met privately)    |
|  - Private liquidation checks (health factor without revealing)  |
|                                                                   |
|  Trust: No single node sees any data. Threshold security.        |
|  Speed: Seconds to minutes (depends on program complexity)       |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
|  Tier 3: ROLLUP (Public execution)                               |
|                                                                   |
|  Sequencer runs BPF VM for public/shared state:                  |
|  - Oracle price updates                                          |
|  - Governance tallying                                           |
|  - Public state that doesn't need privacy                        |
|                                                                   |
|  Trust: Transparent execution, everyone verifies.                |
|  Speed: Microseconds (normal VM execution)                       |
+------------------------------------------------------------------+
```

A single program can span all three tiers. A private DEX program might:
1. **Tier 1** (client): User creates an encrypted order (signs, commits, proves validity)
2. **Tier 2** (MPC): Order matching engine runs on encrypted orders from multiple users
3. **Tier 3** (public): Settlement prices are published, aggregate volume is updated

### System Overview

```
  User A                 User B                 User C
    |                      |                      |
    | encrypted order      | encrypted order      | encrypted order
    v                      v                      v
+------------------------------------------------------------------+
|  MPC PROVER LAYER (Arcium-style MXE cluster)                     |
|                                                                   |
|  Node 1          Node 2          Node 3          Node 4          |
|  [share_A1]      [share_A2]      [share_A3]      [share_A4]     |
|  [share_B1]      [share_B2]      [share_B3]      [share_B4]     |
|  [share_C1]      [share_C2]      [share_C3]      [share_C4]     |
|                                                                   |
|  Jointly execute BPF program on secret-shared inputs:            |
|  - match_orders(encrypted_A, encrypted_B, encrypted_C)           |
|  - No node sees any plaintext order                              |
|  - Result: matched pairs + settlement amounts (encrypted)        |
|                                                                   |
|  Output: commitments + nullifiers + proof of correct execution   |
+------------------------------------------------------------------+
    |
    v
+------------------------------------------------------------------+
|  ROLLUP SEQUENCER                                                |
|                                                                   |
|  - Verify MPC computation proof (on-chain verification)          |
|  - Apply state transitions (new commitments, spent nullifiers)   |
|  - Post to L1 for data availability                              |
+------------------------------------------------------------------+
```

---

## Making BPF Run in MPC

### The Problem

A general-purpose BPF program has conditional branches and dynamic memory access.
Both are catastrophically expensive in MPC because:

1. **Branches leak information**: If you only execute one branch, an observer learns
   which path was taken. In MPC, you must execute BOTH branches and obliviously select
   the result.

2. **Memory access patterns leak information**: If you load `array[secret_index]`,
   the access pattern reveals the index. In MPC, you must use Oblivious RAM (ORAM) —
   touching all elements to hide which one you actually read.

### Solution: Restricted BPF Subset for MPC

Not all BPF programs need to run in MPC. Only specific multi-party functions do.
For these, we define a **restricted BPF subset** that maps efficiently to MPC:

```
MPC-friendly BPF subset:

  ALLOWED (efficient in MPC):
    ✓ ADD, SUB, MUL              — cheap (add is free, mul is 1 round)
    ✓ MOV (register to register) — free (local on shares)
    ✓ Bounded loops (fixed iteration count known at compile time)
    ✓ Poseidon hash syscall      — ~200 multiplications (designed for this)
    ✓ Linear memory access (fixed offsets, not data-dependent)

  RESTRICTED (expensive but supported with overhead):
    ✓ Comparison (JEQ, JGT, etc.) — ~20 multiplications per comparison
    ✓ Conditional select (like a ternary) — evaluate both, MUX result
    ✓ Small lookup tables — O(n) scan instead of random access

  FORBIDDEN (too expensive, must restructure):
    ✗ Dynamic memory indexing (arr[secret_value])
    ✗ Unbounded loops (loop count depends on secret data)
    ✗ Division (very expensive in MPC — ~50+ multiplications)
    ✗ Bit shifts by secret amount
    ✗ SHA-256 (use Poseidon instead)
```

### MPC-BPF Verifier

At deploy time, programs intended for MPC execution go through an additional verifier
that checks they only use the MPC-friendly subset:

```
Standard BPF verifier (Phase 1.8):
  - Jump bounds, register init, r10 read-only, etc.
  - All programs must pass this.

MPC BPF verifier (additional):
  - No dynamic memory indexing (all offsets must be computable at compile time)
  - All loops must have statically-known bounds
  - No forbidden opcodes (div, mod, dynamic shifts)
  - Memory access pattern is data-independent
  - Programs that pass this can run in MPC tier.
```

### MPC Execution Engine

The MPC execution engine wraps our ZBPF VM. Instead of operating on plaintext `u64`
registers, it operates on **secret-shared** field elements:

```
Standard ZBPF:                    MPC-ZBPF:

regs: [u64; 11]                   regs: [Share; 11]
                                   (each Share = node's fragment)

ADD r0, r1:                        ADD r0, r1:
  r0 = r0 + r1                      r0_share = r0_share + r1_share
  (one operation)                    (local, no communication)

MUL r0, r1:                        MUL r0, r1:
  r0 = r0 * r1                      r0_share = beaver_triple_mul(
  (one operation)                        r0_share, r1_share, nodes)
                                     (1 round of communication)

JEQ r0, r1, +5:                    JEQ r0, r1, +5:
  if r0 == r1 { pc += 5 }           bit = secure_compare(r0, r1)
  (branch)                           result_true = execute(pc+5, ...)
                                     result_false = execute(pc+1, ...)
                                     result = mux(bit, result_true,
                                                       result_false)
                                     (execute BOTH paths, select)
```

Each Arx node runs the same MPC-ZBPF engine on its shares. Nodes communicate only
during multiplications and comparisons. The result is a set of output shares that
are reconstructed only when needed (e.g., to produce commitments for the chain).

---

## MPC Prover Layer Integration with Arcium

### Why Arcium Specifically?

Arcium's architecture maps well to our needs:

| Arcium Feature | Our Use |
|----------------|---------|
| **MXEs** (execution environments) | Each MPC-BPF program runs in its own MXE |
| **Configurable clusters** | Users choose how many nodes, which nodes, what trust level |
| **Dishonest majority** | Only 1 honest node needed — strong security with small clusters |
| **Parallel execution** | Multiple MXEs run simultaneously across different clusters |
| **Cheater identification** | Malicious nodes are caught and slashed |
| **Solana integration** | State commitments and proofs verified on-chain |

### Integration Architecture

```
+------------------------------------------------------------------+
|  OUR STACK                                                       |
|                                                                   |
|  +-------------------+    +-------------------+                   |
|  | ZBPF VM           |    | MPC-ZBPF Engine   |                  |
|  | (plaintext exec)  |    | (secret-shared)   |                  |
|  | Tier 1 (client)   |    | Tier 2 (MPC)      |                  |
|  | Tier 3 (rollup)   |    |                   |                  |
|  +--------+----------+    +--------+----------+                   |
|           |                        |                              |
|  +--------v------------------------v----------+                   |
|  | Runtime Layer                              |                   |
|  | - Account model (notes/commitments)        |                   |
|  | - State store (commitment tree, nullifiers) |                  |
|  | - Transaction routing (which tier?)        |                   |
|  +--------------------+-----------------------+                   |
+------------------------------------------------------------------+
                         |
                         v
+------------------------------------------------------------------+
|  ARCIUM LAYER                                                    |
|                                                                   |
|  +-------------------+    +-------------------+                   |
|  | arxOS             |    | Arx Node Cluster  |                  |
|  | Orchestration     |    | [N1][N2][N3][N4]  |                  |
|  | MXE lifecycle     |    | Run MPC-ZBPF on   |                  |
|  | Key management    |    | secret shares      |                  |
|  +-------------------+    +-------------------+                   |
|                                                                   |
|  +-------------------+                                            |
|  | On-chain contract |                                            |
|  | Verify MPC output |                                            |
|  | Slash cheaters    |                                            |
|  | Commit state      |                                            |
|  +-------------------+                                            |
+------------------------------------------------------------------+
```

### Workflow: Private DEX Order Matching

Concrete example showing all three tiers:

```
Phase 1 — Order Submission (Tier 1: Client-side ZK)

  Alice wants to buy 100 TOKEN_A at max price 5.0
  Bob wants to sell 80 TOKEN_A at min price 4.5

  Alice's device:
    1. Run BPF program locally: validate_order(buy, 100, 5.0)
    2. Generate commitment: C_alice = Poseidon(buy, 100, 5.0, salt_a)
    3. Generate ZK proof: "I have funds to cover this order"
    4. Encrypt order for MPC: secret_share(buy, 100, 5.0) -> shares[4]
    5. Submit: {commitment, proof, encrypted_shares} to MPC cluster

  Bob does the same for his sell order.
  Neither Alice, Bob, the sequencer, nor any MPC node sees the other's order.

Phase 2 — Order Matching (Tier 2: MPC Prover Layer)

  MPC cluster (4 Arx nodes) receives encrypted shares from both users.

  Each node holds:
    Node 1: [alice_share_1, bob_share_1]
    Node 2: [alice_share_2, bob_share_2]
    Node 3: [alice_share_3, bob_share_3]
    Node 4: [alice_share_4, bob_share_4]

  Nodes jointly execute MPC-BPF program: match_orders()
    - Compare Alice's max price >= Bob's min price (on shares)
    - Compute fill quantity: min(100, 80) = 80 (on shares)
    - Compute settlement price: (5.0 + 4.5) / 2 = 4.75 (on shares)
    - Generate output commitments for the matched trade

  No node ever sees: Alice's order, Bob's order, the match result,
  or the settlement price. They only see their own shares.

  Output: encrypted result shares -> reconstruct into commitments
    - Nullifiers for Alice's and Bob's order notes
    - New commitments for Alice's TOKEN_A and Bob's payment
    - Proof that the matching was done correctly

Phase 3 — Settlement (Tier 3: Public Rollup)

  Sequencer receives:
    - Nullifiers (orders consumed)
    - Commitments (new notes created)
    - MPC execution proof

  Sequencer:
    1. Verify proof
    2. Check nullifiers not spent
    3. Append commitments to tree
    4. Record nullifiers
    5. Update public state: volume += 80, last_price = 4.75
       (aggregate public data — no individual info leaked)
```

---

## Performance: Client-Side vs MPC Prover Layer

### Is Running a VM "Heavy Computing"?

**No — the VM execution itself is trivial.** A BPF instruction (ADD, MUL, MOV, jump)
is a single CPU operation. Running 10,000 BPF instructions takes ~10 microseconds on
any modern device, including a phone. The VM is not the bottleneck in any tier.

What's expensive is the **privacy layer on top of the VM**:

- **Client-side ZK (Tier 1)**: The VM runs at full speed (~microseconds). Then a
  separate ZK prover step converts the execution trace into a proof. This is slow
  (seconds to minutes) because it involves heavy cryptographic operations (polynomial
  evaluations, FFTs, multi-scalar multiplications). But the VM execution and the proof
  generation are **separate steps** — the VM itself is fast.

- **MPC prover layer (Tier 2)**: The VM runs on secret-shared data. Here the VM
  execution itself is what's slow, because every operation that touches two secret
  values requires cryptographic communication between nodes. There is no separate
  proving step — the MPC protocol guarantees correctness inherently.

### Why MPC Makes Each Instruction Slower

In plaintext execution, a BPF instruction is a single CPU operation. In MPC, the
same instruction may require multiple rounds of communication between nodes.

The fundamental reason: in secret sharing, each node holds a **share** of each value,
not the value itself. Some operations can be done locally on shares (addition), but
others require nodes to talk to each other (multiplication, comparison).

```
Plaintext:                  Secret-shared across 4 nodes:

r0 = 42                    Node 1: r0_share = 11
                            Node 2: r0_share = 7
                            Node 3: r0_share = 15
                            Node 4: r0_share = 9
                            (11 + 7 + 15 + 9 = 42 — but no node knows this)

ADD r0, r1:                 ADD r0, r1:
  r0 = r0 + r1               Each node: r0_share += r1_share
  One CPU instruction         LOCAL — no communication needed
  ~1 nanosecond               ~1 nanosecond (same speed!)

MUL r0, r1:                 MUL r0, r1:
  r0 = r0 * r1               share_a * share_b ≠ share_(a*b)
  One CPU instruction         Nodes must use a "Beaver triple" protocol:
  ~1 nanosecond                 1. Each node uses pre-shared random values
                                2. Nodes exchange masked values (1 round trip)
                                3. Each node computes its share of the product
                                ~1-5 milliseconds (network latency dominates)

JEQ r0, r1, +5:             JEQ r0, r1, +5:
  if r0 == r1, jump            Can't just compare shares — that reveals info.
  One CPU instruction          Must do:
  ~1 nanosecond                  1. Bit decomposition (break into individual bits)
                                 2. Compare bit-by-bit (each bit = 1 multiplication)
                                 3. Execute BOTH branches (can't reveal which taken)
                                 4. Select correct result (MUX operation)
                                 ~10-50 milliseconds

LDXDW r0, [r1+off]:         LDXDW r0, [r1+off]:
  Load from memory             If offset is PUBLIC (fixed): same as plaintext.
  One CPU instruction          If index is SECRET (data-dependent):
  ~1 nanosecond                  Must use ORAM — "touch" ALL memory locations
                                 to hide which one was actually read.
                                 ~1-100 milliseconds (depends on memory size)
```

### Same Program, Different Speeds

Here's a concrete comparison. Same BPF program — a simple order validation with
arithmetic, a comparison, and a conditional return:

```
Program (5 instructions):
  MOV64_IMM r1, 100          // min order size
  LDXDW r2, [r0 + 0]         // load order size from input (fixed offset)
  JGE r2, r1, +1             // if order_size >= 100, skip error
  MOV64_IMM r0, 1            // return error
  EXIT                        // return r0
```

| Step | Client (Tier 1) | MPC (Tier 2) | Why different |
|------|----------------|-------------|---------------|
| MOV64_IMM r1, 100 | ~1 ns | ~1 ns | Immediate value, no secrets involved |
| LDXDW r2, [r0+0] | ~1 ns | ~1 ns | Fixed offset — not data-dependent |
| JGE r2, r1, +1 | ~1 ns | ~15 ms | Comparison on secret shares |
| MOV64_IMM r0, 1 | ~1 ns | ~1 ns | But BOTH this AND next line execute |
| EXIT | ~1 ns | ~1 ns + MUX ~3 ms | Select which branch result to use |
| **VM execution** | **~5 ns** | **~20 ms** | **~4,000,000x slower** |
| ZK proof gen | ~10 seconds | N/A | MPC doesn't need a separate proof |
| **End-to-end** | **~10 seconds** | **~20 ms** | **MPC wins for tiny programs** |

For this tiny program, MPC is actually faster end-to-end because it avoids the ZK
proof generation step entirely. The MPC protocol inherently proves correctness.

Now a larger program — token transfer with balance checks, hash operations, and
Merkle proof verification (~2,000 instructions):

| Metric | Client (Tier 1) | MPC (Tier 2) |
|--------|----------------|-------------|
| VM execution | ~2 microseconds | ~10-30 seconds |
| ZK proof generation | ~15-30 seconds | N/A |
| **End-to-end** | **~15-30 seconds** | **~10-30 seconds** |

Comparable. The ZK proof overhead and MPC execution overhead are in the same ballpark.

Now a complex program — DEX matching engine processing 50 orders with sorting and
price-time priority (~50,000 instructions):

| Metric | Client (Tier 1) | MPC (Tier 2) |
|--------|----------------|-------------|
| VM execution | ~50 microseconds | ~5-20 minutes |
| ZK proof generation | ~2-5 minutes | N/A |
| **End-to-end** | **~2-5 minutes** | **~5-20 minutes** |

MPC is slower for large programs because every instruction pays the communication cost.
ZK proof generation also scales with program size, but the per-instruction cost is lower
(constraints are cheaper than network round trips).

### The Real Tradeoff: Speed vs Trust Model

The choice between client-side and MPC is **not primarily about speed**. It's about
**who is allowed to see the data**:

```
Client-side ZK:
  ✓ User sees ALL their own data (inputs, outputs, execution state)
  ✓ Nobody else sees anything (just the proof)
  ✗ Requires the user to be online and have compute power
  ✗ Can't handle multi-party computation (who runs the matching?)

  Use when: You're operating on YOUR OWN data.
  Example:  Transferring your own tokens privately.

MPC prover layer:
  ✓ NO SINGLE PARTY sees ANY data (not even the submitter)
  ✓ Handles multi-party computation natively
  ✓ No heavy client-side proving (good for mobile/lightweight clients)
  ✗ Slower per-instruction (network communication overhead)
  ✗ Requires a live cluster of honest nodes

  Use when: Multiple users' data must be combined without anyone seeing it.
  Example:  Matching buy and sell orders from different users.
```

A user making a simple transfer gains nothing from MPC — they already know their own
balance. Client-side ZK is faster and sufficient.

A DEX matching encrypted orders from 50 different users MUST use MPC — no single
party should see all 50 orders. The speed penalty is the price of that trust model.

### When They Converge

For programs under ~500 instructions (simple validations, basic comparisons, small
state updates), MPC and client-side ZK have similar end-to-end latency:

```
End-to-end latency (rough):

Instructions    Client-side ZK    MPC Prover Layer
     10          ~5-10 seconds       ~50 ms
    100          ~5-10 seconds      ~500 ms
    500          ~10-15 seconds     ~3-5 seconds
   1000          ~15-20 seconds     ~10-30 seconds
   5000          ~30-60 seconds     ~1-5 minutes
  50000          ~2-5 minutes       ~10-30 minutes

Crossover point: ~500-2000 instructions
Below this: MPC is faster (no proof generation overhead)
Above this: Client-side ZK is faster (lower per-instruction cost)
```

The sweet spot for MPC: programs that are **small but multi-party**. Keep the
matching logic simple, do the heavy validation client-side (ZK), and use MPC only
for the part that requires seeing multiple users' data.

### Optimization: Preprocessing

MPC has an offline/online split. Expensive cryptographic material (Beaver triples,
correlated randomness) can be **precomputed** before the actual inputs arrive:

```
Offline phase (before users submit orders):
  - Generate Beaver triples for multiplications
  - Generate correlated randomness for comparisons
  - This is slow but happens in advance
  - Can run during cluster idle time

Online phase (when users submit orders):
  - Use precomputed material for fast execution
  - Only lightweight communication between nodes
  - 10-100x faster than doing everything online

Impact on the numbers above:
  With preprocessing, the 500-instruction crossover point moves to ~2000-5000
  instructions, making MPC competitive for larger programs.
```

Arcium's MXE infrastructure supports this — clusters can precompute material during
idle time, then execute computations quickly when requests arrive.

### Optimization: Hybrid Decomposition

The most practical optimization: **don't run the entire program in MPC**. Split it:

```
Private DEX example — naive approach (everything in MPC):
  50,000 instructions, all on secret shares
  → 10-30 minutes in MPC

Hybrid approach:
  Step 1 — Client-side (Tier 1): each user validates their own order
           (balance check, signature, format)
           ~2000 instructions per user × 50 users
           Each user proves their order is valid → ZK proof
           ~15 seconds per user (parallel)

  Step 2 — MPC (Tier 2): matching engine compares validated orders
           Only the COMPARISON logic runs in MPC
           ~200 instructions (just the core matching)
           ~1-3 seconds in MPC

  Step 3 — Client-side (Tier 1): each matched user generates settlement proof
           ~1000 instructions per user
           ~10 seconds per user (parallel)

Total: ~30 seconds (limited by slowest client proof)
vs 10-30 minutes (everything in MPC)

Speedup: 20-60x
```

This is why the three-tier model matters — you put each computation where it's
cheapest and only escalate to MPC for the multi-party-critical piece.

### When to Use Each Tier

```
Decision tree:

  Is the computation single-user?
    YES → Tier 1 (Client-side ZK)
          Fastest. User already knows their data. Just prove it.

    NO → Does it involve multiple users' private data?
      YES → Is it a simple operation (< 500 BPF instructions)?
        YES → Tier 2 (MPC Prover Layer)
              MPC handles it in seconds. No single party sees data.

        NO → Can it be broken into small MPC steps + client ZK?
          YES → Hybrid (Tier 1 + Tier 2)
                Users prove their parts (ZK), MPC does the multi-party part.
                This is almost always the right answer for complex operations.

          NO → Tier 2 with optimized MPC-BPF program
               Accept the latency or optimize the program.

      NO → Tier 3 (Public Rollup)
           No privacy needed. Just run it.
```

---

## Intent Protection Analysis

The original requirement: **intent can't be leaked**. Here's how each tier handles it:

### Tier 1 (Client-Side ZK): Intent Hidden From Everyone Except User

```
What the user knows:         Everything (it's their transaction)
What the sequencer sees:     Nullifiers, commitments, proof (opaque bytes)
What L1 sees:                Same as sequencer
What MPC nodes see:          Nothing (not involved)

Intent leakage:              Zero (from external observers)
Risk:                        User's device compromise leaks intent
```

### Tier 2 (MPC Prover Layer): Intent Hidden From EVERYONE

```
What each MPC node sees:     One share of the input (meaningless alone)
What the sequencer sees:     Nullifiers, commitments, proof (opaque bytes)
What L1 sees:                Same as sequencer
What the user sees:          Only their own input, not others'

Intent leakage:              Zero (even from the user regarding others' data)
Risk:                        Collusion of ALL nodes (dishonest majority:
                             only if every single node is compromised)
```

### Tier 3 (Public): Intent Fully Visible

```
Everything is transparent. Use only for data that doesn't need privacy.
```

### Combined: A Transaction That Spans Tiers

```
Private DEX swap:
  1. User creates order intent (Tier 1)
     → User knows their own order. Nobody else does.
  2. Order enters matching engine (Tier 2)
     → MPC nodes have shares. No node knows any order.
     → Matching happens on encrypted data.
  3. Settlement published (Tier 3)
     → Only aggregate: "80 tokens traded at 4.75"
     → No individual order details.

Intent at every stage:        HIDDEN
Linkability:                  User -> order: only user knows
                              Order -> match: nobody knows
                              Match -> settlement: only aggregates published
```

---

## Linkability Minimization in MPC Context

MPC adds new linkability concerns beyond what the ZK-only architecture has:

### Concern: MPC Cluster Membership

If Alice always uses the same MPC cluster, the cluster nodes learn "Alice submits
something every Tuesday at 3pm" even without seeing the contents.

**Mitigation**: Rotate clusters. Use different Arx node subsets for each computation.
Arcium supports dynamic cluster formation — request a fresh cluster each time.

### Concern: Timing Correlation

If Alice submits an order and 200ms later an MPC computation starts, the timing
correlation might link Alice to that computation.

**Mitigation**: Batching. Collect orders over a time window (e.g., 1 second), then
run the matching computation on the entire batch. Any of the N users in the batch
could be the source of any order.

### Concern: Input Size Side Channel

If Alice's order involves a complex multi-leg strategy, her encrypted input might be
larger than Bob's simple limit order.

**Mitigation**: Fixed input sizes. All orders are padded to the same size before
encryption and secret-sharing. Dummy fields are filled with random data.

### Concern: Computation Time Side Channel

If a matching engine takes longer when there are more matches, an observer could
infer the match rate.

**Mitigation**: Fixed computation time. The MPC-BPF program always runs for the
maximum number of steps regardless of the actual workload (pad with dummy operations).

---

## Comparison: ZK-Only vs MPC vs Hybrid

| Aspect | ZK-Only (privacy-architecture.md) | MPC-Only | Hybrid (this document) |
|--------|----------------------------------|----------|----------------------|
| **Who sees data** | User sees their own | Nobody | User sees own; MPC for multi-party |
| **Multi-party compute** | Not possible | Native | Yes (Tier 2) |
| **Single-user privacy** | Strong (ZK proof) | Overkill (MPC overhead) | Strong (Tier 1 = ZK) |
| **Latency** | Proof gen: 5-60s | Every op is slow | Best of both |
| **Complexity** | Moderate | Very high | High |
| **Trust model** | Trust your own device | Trust threshold of nodes | Both |
| **General-purpose VM** | Restricted (ZK-friendly) | Restricted (MPC-friendly) | Full VM for testing + restricted for production |
| **Existing infra** | Build from scratch | Arcium/Nillion/Partisia | Leverage Arcium for Tier 2 |

**The hybrid model is the pragmatic choice.** Most operations are single-user (Tier 1,
cheap), multi-party operations use MPC (Tier 2, available via Arcium), and public state
uses the standard VM (Tier 3, fastest).

---

## Open Questions

### 1. Arcium Integration Depth

How tightly do we integrate with Arcium?

- **Light**: Use Arcium's MXE as a black box. Our MPC-BPF compiles to Arcis programs
  that run on their infrastructure. We don't touch their internals.
- **Deep**: Build a native MPC-ZBPF engine as an Arcium-compatible Arx node module.
  Our VM runs directly inside their node infrastructure.
- **Fork/Inspired**: Build our own MPC prover layer inspired by Arcium's architecture
  but tailored for BPF execution.

### 2. Secret Sharing Scheme

What kind of secret sharing for the MPC layer?

- **Shamir** (t-of-n threshold): Flexible, well-studied. Used by Arcium.
- **Replicated**: Faster for small parties (3 nodes) but doesn't scale.
- **Additive**: Simplest, but no threshold — all parties needed.

### 3. BPF-to-Arcis Compilation

If we integrate with Arcium, we need a compiler path:
```
C source → BPF bytecode → MPC-BPF subset → Arcis program
```
Or alternatively:
```
C source → Arcis directly (bypass BPF for MPC programs)
```
The first preserves our "everything is BPF" philosophy. The second is more practical.

### 4. State Model Compatibility

The note model (commitments + nullifiers) must work across all three tiers.
A note created in Tier 1 (client ZK) must be spendable in Tier 2 (MPC), and
vice versa. This means the commitment scheme and Merkle tree must be shared.

### 5. Proof Composability

When MPC produces a result, how is it verified?

- **MPC output + ZK proof**: The MPC cluster generates a ZK proof that computation
  was correct. Verifiable by anyone. This is the gold standard but adds proving cost.
- **MPC output + fraud proof**: Assume correctness, allow challenges. Cheaper but
  has a challenge window (optimistic).
- **MPC output + threshold signatures**: The cluster signs the result. Trust the
  threshold. Cheapest but weaker guarantees.

Arcium uses a combination of on-chain verification + cheater identification with
slashing. We could extend this with ZK proofs for critical computations.
