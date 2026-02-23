# State Freshness & Composability in a Privacy Layer

How a client-side VM keeps up with on-chain state, and why composability works
differently (not worse) in a privacy architecture.

---

## The State Freshness Problem

On a validator, the VM has direct access to the full current state — every account,
every balance, updated in real-time. When the VM moves client-side, the client must
fetch state from the chain, and that state can go stale.

### The Problem (Account Model)

```
Time 0.0s:  Client fetches account state (Pool.balance = 1000)
Time 0.1s:  Someone else's transaction modifies Pool.balance → 950
Time 15.0s: Client finishes ZK proof (based on Pool.balance = 1000)
Time 15.1s: Client submits proof to sequencer
            → INVALID: proof assumes Pool.balance = 1000, but it's 950
            → 15 seconds of proving wasted. Must retry.
```

This is **state contention**. In Solana's account model, popular accounts (DEX pools,
token mints, lending reserves) are modified by many users constantly. Any client-side
proof that touches a high-contention account will frequently be invalidated by other
transactions that land first.

This is why Solana's account model and client-side proving are a bad fit — the
combination of shared mutable state + slow proving creates a retry loop.

### How The Note Model Solves This

Notes are **not shared mutable state**. They are immutable, one-time-use, and owned
by a single user. There is no contention because no two users ever modify the same
note.

```
Account model (contention):
  Alice reads:  Pool.balance = 1000
  Bob reads:    Pool.balance = 1000
  Alice proves: Pool.balance → 950 (she swapped 50)
  Bob proves:   Pool.balance → 970 (he swapped 30)

  Alice submits first → accepted
  Bob submits → INVALID (pool is now 950, not 1000)
  Bob wasted 15 seconds. Must re-fetch, re-execute, re-prove.

Note model (no contention):
  Alice owns: Note_A {value: 100, salt: 0xabc}
  Bob owns:   Note_B {value: 200, salt: 0xdef}

  Alice proves: spend Note_A → create Note_C + Note_D
  Bob proves:   spend Note_B → create Note_E + Note_F

  Alice submits → accepted (Note_A nullified)
  Bob submits → accepted (Note_B nullified)

  No contention. Both proofs valid. They touch DIFFERENT notes.
  The only collision possible: Alice and Bob spend the SAME note.
  That's a double-spend — caught by the nullifier set, not a state
  contention problem.
```

Notes work like UTXOs in Bitcoin — each is independent, owned by one person, consumed
once. Your proof is valid as long as your note hasn't been spent by someone else,
which only happens if your private key is compromised.

**State contention is eliminated by construction**, not by optimistic retries.

### Accessing Public State

Private programs sometimes need to read public state (e.g., a private swap needs the
current oracle price). This uses **Merkle proofs**:

```
Client-side:
  1. Fetch public state value (e.g., oracle_price = 4.75)
  2. Fetch Merkle proof of that value against the current public state root
  3. Include both as private inputs to the ZK circuit
  4. Circuit verifies: MerkleProof(oracle_price, path, public_state_root) == true

Sequencer checks:
  - public_state_root is from the current block or within the last N blocks
  - If the root is too old, reject (prevents proving against ancient state)
```

Public state changes relatively slowly (oracle prices update every few seconds, not
every millisecond). A staleness window of a few blocks is almost always sufficient.
If the public state your proof depends on changed between fetch and submission, the
proof fails — but this is rare for read-only public state.

### Staleness Windows

| State Type | Contention Risk | Staleness Window | Strategy |
|-----------|----------------|-----------------|----------|
| User's own notes | Zero (you're the only spender) | Infinite (immutable) | No problem |
| Other users' notes | Zero (they spend theirs, you spend yours) | N/A | No problem |
| Public state (oracle) | Low (changes slowly) | ~5-10 blocks | Merkle proof + root freshness check |
| Public state (high-contention pool) | Medium | ~1-2 blocks | Merkle proof, retry if stale |
| Shared mutable account | High | Immediate | Not used — note model avoids this |

---

## Composability in a Privacy Layer

### The Misconception

"You can't have composability with client-side execution because CPI breaks."

This is wrong. What breaks is one specific form of composability — synchronous
reads/writes to shared mutable state between different users. But:

1. That's not the most common form of composability
2. The note model enables different (and arguably cleaner) patterns
3. MPC (Tier 2) handles the multi-party cases

### Single-User CPI — Works Identically

The most common DeFi composability pattern: a single user calling a chain of programs
that operate on that user's assets.

```
Solana (transparent, synchronous):
  Your tx → DEX::swap() → Token::transfer() → System::debit()
  Validator executes all three programs atomically in one transaction.

Our architecture (private, synchronous):
  Your proof → DEX::swap() → Token::transfer() → System::debit()
  Client executes all three programs in the VM and proves the chain.
  Same atomicity. Same composability. Nobody else sees it.
```

This works because **you own all the notes involved**. You have all the state locally.
The VM runs the entire CPI chain, and the ZK proof covers the full execution trace.

On Solana, CPI programs can't modify accounts you didn't sign for. The authorization
model already requires your signature for state changes to your accounts. In the note
model, your secret key plays the same role — you can only spend notes you own.

**Examples that work as single-user CPI:**

| Operation | Solana CPI | Private CPI | Difference |
|-----------|-----------|-------------|------------|
| Swap tokens on DEX | swap() → transfer() → transfer() | Same chain in one proof | None |
| Deposit into lending | deposit() → transfer() → mint_receipt() | Same chain in one proof | None |
| LP provision | add_liquidity() → transfer_A() → transfer_B() → mint_lp() | Same chain in one proof | None |
| Stake tokens | stake() → transfer() → update_stake_account() | Same chain in one proof | None |
| Flash loan | borrow() → use_funds() → repay() | Same chain in one proof | None |

**Flash loans deserve special attention**: The entire borrow-use-repay cycle happens
within a single proof. The circuit enforces that repayment equals or exceeds the
borrow amount. This is identical to Solana's model where flash loans must repay within
the same transaction — the proof boundary replaces the transaction boundary.

### Reading Public State — Works Via Merkle Proofs

Programs often need to read shared state (oracle prices, pool parameters, governance
settings). In a privacy layer, this uses Merkle proofs against the public state tree:

```
Solana:
  DEX::swap() reads oracle_price from Oracle account (direct state access)

Private:
  User fetches oracle_price + Merkle proof from public state tree
  Circuit verifies: MerkleProof(oracle_price, path, state_root)
  DEX::swap() uses the verified oracle_price

  Difference: one extra Merkle verification (~10,000 constraints)
  Not free, but cheap. And the oracle price is public anyway.
```

### Multi-User Atomic Operations — Async Instead of Sync

This is where the pattern changes. When multiple users' private state must be
combined atomically, no single user can prove the whole thing.

**On Solana**: Both users sign one transaction, validator executes atomically.

**In our architecture**: The composition happens in stages. Two patterns:

#### Pattern 1: Escrow Notes (No MPC Needed)

Conditional notes encode swap logic directly. No trusted third party required.

```
Step 1 — Alice creates an escrow note (Tier 1, client-side):

  Escrow note: {
    value: 50 TOKEN_A,
    condition: "claimable by anyone who sends 10 TOKEN_B to
                stealth_address_X within 100 blocks"
    refund: "if unclaimed after 100 blocks, refundable to Alice"
  }

  Alice proves: she owns 50 TOKEN_A and the escrow note is well-formed.
  Escrow commitment is published on-chain.

Step 2 — Bob claims the escrow (Tier 1, client-side):

  Bob's proof covers BOTH sides of the swap atomically:
    a. "I'm spending 10 TOKEN_B to stealth_address_X" (Alice's address)
    b. "I'm claiming the escrow note's 50 TOKEN_A"
    c. "The escrow conditions are met" (correct amount, within time limit)

  One proof. Both transfers happen or neither does. Atomic.

Step 3 — If unclaimed:

  After 100 blocks, Alice proves the escrow expired and reclaims her funds.
```

This is how atomic swaps work in Zcash and Bitcoin (Hash Time-Lock Contracts), but
generalized. The note's `app_data` encodes arbitrary conditions that the claiming
proof must satisfy.

**Composability here is real** — Bob's proof composes the escrow program with the
token transfer program in a single atomic proof. It's just initiated by a different
user (Bob) than the one who created the escrow (Alice).

#### Pattern 2: MPC Matching (Tier 2)

For operations involving many users simultaneously (orderbook matching, auctions):

```
Step 1 — Each user creates an encrypted order (Tier 1, parallel):

  Alice: "buy 100 TOKEN_A at max 5.0" → ZK proof + encrypted commitment
  Bob:   "sell 80 TOKEN_A at min 4.5"  → ZK proof + encrypted commitment
  Carol: "buy 50 TOKEN_A at max 4.8"   → ZK proof + encrypted commitment

  Each user proves their order is valid (they have funds, parameters are
  well-formed). ~15 seconds per user, all in parallel.

Step 2 — MPC cluster matches orders (Tier 2):

  Arx nodes receive secret-shared orders from all users.
  Jointly execute matching engine on encrypted data.
  No node sees any order.

  Output: settlement commitments for matched pairs.
  Alice gets TOKEN_A, Bob gets payment. Carol's order partially filled.

  ~1-3 seconds (small matching program).

Step 3 — Settlement notes appear on-chain:

  New note commitments published. Each user scans for their notes.
  Atomic: all settlements from a match happen together or not at all.
```

**This IS composability** — multiple programs (order validation, matching engine,
settlement) compose across tiers. It's asynchronous (commit → match → settle)
instead of synchronous (one tx), but the atomicity guarantees are the same.

### Flash Loans — A Detailed Example

Flash loans are often cited as the ultimate composability test. They work fine:

```
Solana flash loan:
  1. borrow(1000 USDC) from LendingPool
  2. swap(1000 USDC → 1.1 ETH) on DEX_A
  3. swap(1.1 ETH → 1050 USDC) on DEX_B  (arbitrage)
  4. repay(1000 USDC + 5 USDC fee) to LendingPool
  5. keep 45 USDC profit
  All in one transaction. If repay fails, everything reverts.

Private flash loan:
  1. borrow(1000 USDC note) from LendingPool
  2. swap(1000 USDC note → 1.1 ETH note) via DEX_A
  3. swap(1.1 ETH note → 1050 USDC note) via DEX_B
  4. repay(1000 USDC + 5 fee note) to LendingPool
  5. keep 45 USDC note as profit
  All in one proof. Circuit enforces repayment ≥ borrow + fee.
  If repayment constraint fails, proof is invalid (can't even generate it).

  The "revert" is even stronger — you can't create an invalid proof.
  On Solana, the revert happens at execution time. In ZK, invalid
  executions can't be proven at all.
```

The user executes the entire CPI chain in their local VM. The lending pool's "flash
loan" program checks that the same proof includes a repayment. Since the proof covers
the full execution trace, the repayment is guaranteed by the circuit constraints.

**Note**: The DEX pools here could be public state (Tier 3 Merkle proofs) or note-based
private pools. Either way, the user proves the full chain client-side.

---

## The Full Composability Matrix

| Composability Pattern | Solana | Tier 1 (Client ZK) | Tier 2 (MPC) | Tier 3 (Public) |
|---|---|---|---|---|
| **Single-user CPI** (swap, lend, LP, stake) | Sync, atomic | Sync, atomic (one proof) | N/A | Sync, atomic |
| **Flash loans** (borrow + use + repay) | Sync, atomic | Sync, atomic (one proof) | N/A | Sync, atomic |
| **Read public state** (oracle, params) | Direct access | Merkle proof | Merkle proof | Direct access |
| **Two-user atomic swap** | One signed tx | Escrow notes | MPC matching | One signed tx |
| **Multi-user matching** (orderbook) | On-chain program | N/A (can't see others' data) | MPC cluster | On-chain program |
| **Cross-user private state read** | N/A (all public) | N/A | MPC only | N/A (all public) |
| **Program A → Program B → Program C** | CPI, synchronous | CPI in one proof | CPI in MPC | CPI, synchronous |

### What We Gain That Solana Can't Do

The privacy layer enables composability patterns that are **impossible** on transparent
chains:

| Pattern | Transparent Chain | Privacy Layer |
|---------|------------------|---------------|
| Sealed-bid auction (nobody sees bids) | Impossible (all bids visible) | Tier 2 MPC |
| Dark pool (private orderbook) | Impossible (orderbook visible) | Tier 2 MPC |
| Private lending (health factor hidden) | Impossible (positions visible) | Tier 2 MPC checks |
| Conditional reveal (show balance > X without showing balance) | Impossible | Tier 1 ZK proof |
| Private voting (vote without revealing choice) | Commit-reveal (leaks on reveal) | Tier 1 ZK proof |

---

## Why The Note Model Is Cleaner Than Shared Mutable State

The deeper insight: the note model doesn't just avoid state contention — it's a
fundamentally cleaner composability model.

### Shared Mutable State (Account Model)

```
Problem: Race conditions, contention, ordering-dependent behavior.

  Alice and Bob both try to swap on the same DEX pool.
  Whoever lands first changes the price for the other.
  Validator ordering determines who gets a better price.
  → MEV, front-running, sandwich attacks.

  Composability works, but execution ORDER matters.
  Validators have power over ordering → extractable value.
```

### Immutable Notes (Note Model)

```
No race conditions. No ordering dependence for single-user operations.

  Alice's swap consumes her notes and creates new notes.
  Bob's swap consumes his notes and creates new notes.
  Neither affects the other (different notes).

  For multi-user operations (matching), the MPC cluster determines
  the match result, but individual note operations are independent.

  No single entity controls ordering of single-user transactions
  because there's nothing to order against — no shared state to race for.
  → MEV is structurally eliminated for single-user operations.
```

The note model is more like the **actor model** in concurrent programming (isolated
state, message passing) versus the **shared memory** model (locks, races, contention).
Both achieve composability, but the actor model avoids entire classes of bugs.

---

## Summary

| Question | Answer |
|----------|--------|
| How does client-side VM keep up with state? | Note model eliminates contention. Public state via Merkle proofs with freshness windows. |
| Does composability work in a privacy layer? | Yes. Single-user CPI is identical. Multi-user ops are async (escrow notes or MPC). |
| Is it worse than Solana's composability? | Different, not worse. Gains privacy + MEV resistance. Loses synchronous multi-user atomicity (replaced by escrow/MPC). |
| What about flash loans? | Work in one proof. Circuit enforces repayment. Stronger than runtime revert (invalid proofs can't be generated). |
| What's impossible on Solana that we can do? | Private multi-party compute (sealed bids, dark pools, private health checks) via MPC. |
