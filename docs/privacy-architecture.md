# Privacy Architecture — Deep Dive

This document is the deep dive on the privacy and ZK layer. It extends the system
overview in architecture.md and the MPC details in mpc-prover-layer.md.

- **architecture.md** — three-tier execution model, state model, VM design, system overview
- **mpc-prover-layer.md** — MPC execution details, Arcium integration, performance analysis
- **this document** — ZK proving, note model internals, circuits, linkability analysis,
  threat model, compliance

---

## Threat Model

### What We Protect Against

| Threat | Description | Which tier defends |
|--------|-------------|-------------------|
| **Transaction graph analysis** | Linking sender to receiver across transactions | All tiers (notes + stealth addresses) |
| **Balance tracking** | Observing how much someone holds or transfers | Tier 1 + 2 (commitments hide values) |
| **Intent front-running** | Seeing what someone intends before execution | Tier 1 (local exec), Tier 2 (MPC) |
| **Program identification** | Knowing which program was invoked | Tier 1 + 2 (proof doesn't reveal program) |
| **Behavioral fingerprinting** | Identifying users by transaction patterns | Fixed shapes + timing mitigations |
| **Sequencer knowledge** | Sequencer learning private details | All tiers (sequencer sees only proofs) |
| **MPC node collusion** | MPC nodes combining shares to reconstruct data | Tier 2 (dishonest majority — all nodes must collude) |
| **Metadata correlation** | Timing, size, gas as side channels | Fixed sizes + fees + batching |

### What We Do NOT Protect Against

| Non-goal | Reason |
|----------|--------|
| **Network-level anonymity** | Use Tor/mixnets for IP privacy (separate layer) |
| **Endpoint security** | Compromised device = no privacy regardless |
| **Voluntary disclosure** | Users can choose to make transactions public |
| **Quantum attacks** | Current ZK schemes are pre-quantum; upgrade path exists |
| **All-node MPC collusion** | If every Arx node is compromised, Tier 2 breaks (but Tier 1 is unaffected) |

### Trust Assumptions

| Component | Trust Level | What happens if compromised |
|-----------|------------|---------------------------|
| **ZK proving system** | Mathematical (hardest to break) | All privacy breaks — existential failure |
| **User's device** | User's responsibility | That user's Tier 1 privacy breaks, others unaffected |
| **Sequencer** | **Untrusted** — sees only proofs + ciphertext | Cannot break privacy. Can censor (liveness issue, not privacy) |
| **MPC cluster (Tier 2)** | Threshold trust: 1-of-N honest nodes sufficient | All nodes must collude to break a single computation's privacy |
| **L1 (data availability)** | Public — everything posted there is visible | By design. Only proofs + encrypted data posted |
| **Other users** | Untrusted | Cannot learn anything about your transactions |

---

## How Privacy Works Across The Three Tiers

The three-tier execution model is defined in architecture.md. Here we focus on
the privacy properties and ZK mechanics of each tier.

### Tier 1: Client-Side ZK Execution

```
What happens:
  User runs BPF VM on their own device (plaintext, fast)
  → generates a ZK proof that execution was correct
  → submits proof + encrypted outputs to sequencer

Who sees what:
  User:         Everything (their own data — this is acceptable)
  Sequencer:    Nullifiers + commitments + proof (opaque bytes)
  MPC nodes:    Nothing (not involved)
  L1:           Same as sequencer
  Other users:  Nothing
```

**When to use**: Single-user operations. The user already knows their own data
(their balance, their transfer amount, their recipient). No need for MPC overhead.
Client-side ZK hides this from everyone else.

**Examples**: Private transfers, private state mutations, note creation/spending.

**Performance**: VM execution is ~microseconds (plaintext). ZK proof generation is
~5-60 seconds (the bottleneck). See mpc-prover-layer.md for detailed comparison.

### Tier 2: MPC Execution

```
What happens:
  Multiple users secret-share their inputs to an MPC cluster
  → Arx nodes jointly execute MPC-BPF on secret shares
  → No node sees any plaintext
  → Output: commitments + nullifiers + correctness proof

Who sees what:
  Each MPC node:  One share per input (meaningless alone)
  User A:         Only their own inputs (not user B's)
  User B:         Only their own inputs (not user A's)
  Sequencer:      Nullifiers + commitments + proof (opaque bytes)
  L1:             Same as sequencer
```

**When to use**: Multi-party operations where no single party should see the combined
data. Client-side ZK fails here because *someone* has to run the computation on all
inputs — and that someone would see everything.

**Examples**: Order matching (sealed bids), private auctions, cross-user state reads,
private liquidation checks.

**Performance**: Per-instruction MPC overhead (1-50ms per operation depending on type).
See mpc-prover-layer.md for detailed cost breakdown and optimization strategies.

### Tier 3: Public Execution

```
What happens:
  Sequencer runs BPF VM on public state (transparent, no privacy)

Who sees what:
  Everyone sees everything. Same as Solana's model.
```

**When to use**: Public state that doesn't need privacy. Oracle prices, governance
tallying, aggregate statistics.

### Combined: A Transaction Spanning All Tiers

```
Private DEX swap — full lifecycle:

  Tier 1 (each user, parallel):
    Alice creates encrypted buy order  → ZK proof of validity
    Bob creates encrypted sell order   → ZK proof of validity
    ~15 seconds each (parallel, so 15 seconds total)

  Tier 2 (MPC cluster):
    Matching engine runs on secret-shared orders
    Compares prices, computes fill quantity and settlement
    No node sees any order → only output commitments
    ~1-3 seconds (small matching program, ~200 instructions)

  Tier 3 (sequencer):
    Update public price oracle: last_price = 4.75
    Update public volume counter: volume += 80
    No individual trade details leaked

  Intent visibility at each stage:
    Alice's order intent:  only Alice knows
    Bob's order intent:    only Bob knows
    Match result:          nobody knows until settlement commitments are published
    Public aggregates:     everyone knows (by design)
```

---

## The Note Model — Detailed Design

### Note Structure

```rust
struct Note {
    owner: [u8; 32],       // public key of the owner (stealth address)
    value: u64,            // amount
    asset_type: [u8; 32],  // token identifier
    salt: [u8; 32],        // random nonce (makes each note unique)
    app_data: Vec<u8>,     // arbitrary program-specific state
}
```

A note is **never stored on-chain in plaintext**. The chain stores only the commitment:

```
commitment = PoseidonHash(owner || value || asset_type || salt || app_data)
```

The owner stores the full note locally (in an encrypted local database). They need
all fields to prove ownership and spend the note later.

### Commitment Scheme

**Poseidon hash** is the primary commitment scheme. It takes the note fields as inputs
and produces a single 32-byte output. Properties:

- **Hiding**: Given a commitment, you can't recover the inputs (one-way function)
- **Binding**: You can't find two different notes that produce the same commitment
  (collision resistance)
- **ZK-friendly**: ~200-300 arithmetic constraints per hash (vs ~30,000 for SHA-256)
- **MPC-friendly**: ~200 multiplications per hash (efficient in Tier 2 as well)

**Pedersen commitments** are used where **additive homomorphism** is needed:

```
Pedersen: Commit(v, r) = v*G + r*H    (G, H are generator points)

Property: Commit(a, r1) + Commit(b, r2) = Commit(a+b, r1+r2)

Use case: Verify value conservation WITHOUT revealing amounts.
  sum(input_commitments) - sum(output_commitments) - fee_commitment = 0
  This checks that total inputs = total outputs + fee
  without revealing any individual value.
```

### Nullifier Scheme

```
nullifier = PoseidonHash(commitment, owner_secret_key)
```

Properties:
- **Deterministic**: Same note always produces the same nullifier (prevents double-spend)
- **Unlinkable**: Without `owner_secret_key`, you can't compute the nullifier from the
  commitment or vice versa (different hash domains)
- **Unique**: Each note has exactly one nullifier (no aliasing)

**Why the nullifier requires the secret key**: If nullifier = Hash(commitment), anyone
could compute it and link spending to creation. By mixing in the secret key, only the
owner can produce the nullifier, and observers can't link it to the commitment.

### Commitment Merkle Tree

Append-only sparse Merkle tree. Depth 32 (~4 billion leaves). Poseidon hash at every level.

```
                     Root (public)
                    /            \
                H(0,1)          H(2,3)
               /      \       /      \
            H(0)    H(1)   H(2)    H(3)
             |       |       |       |
            C_0     C_1     C_2     C_3    ← note commitments (leaves)
```

**Membership proof**: A path of 32 sibling hashes from leaf to root. The prover
shows "I know a leaf and a path such that recomputing the root matches the public
root." This proves the note exists without revealing which leaf it is.

**Cost in circuit**: 32 levels × ~300 constraints per Poseidon hash = ~10,000 constraints.

**Append-only**: Leaves are never removed or modified. Spent notes remain in the tree
forever — they're just marked as spent via nullifiers. This simplifies the tree
structure and avoids complex rebalancing.

### Nullifier Set

A simple set (HashSet or sparse Merkle tree) of all published nullifiers. On each
transaction, the sequencer checks:

1. Are any of these nullifiers already in the set? → Reject (double-spend attempt)
2. If not, add them to the set

If using a sparse Merkle tree for the nullifier set, non-membership proofs are possible
(prove a nullifier has NOT been used). This is useful for certain privacy protocols.

---

## ZK Circuits — What Gets Proven

### Circuit for a Private Transaction (Tier 1)

The ZK proof for a client-side private transaction proves:

```
PUBLIC INPUTS (known to sequencer/verifier):
  - nullifiers[0..N]          the nullifiers being published
  - commitments[0..N]         the new note commitments
  - merkle_root               the commitment tree root used
  - fee                       transaction fee (public signal)

PRIVATE INPUTS (known only to prover/user):
  - input_notes[0..N]         full note data (value, owner, salt, app_data)
  - input_merkle_paths[0..N]  Merkle proof for each input note
  - owner_secret_key          proves ownership of input notes
  - output_notes[0..N]        full note data for newly created notes

CONSTRAINTS:

  1. Input note ownership and existence:
     For each input note i:
       a. commitment_i = PoseidonHash(input_notes[i])
       b. MerkleVerify(commitment_i, merkle_path_i, merkle_root) == true
       c. nullifier_i = PoseidonHash(commitment_i, owner_secret_key)
       d. nullifier_i matches the public nullifiers[i]

  2. Output note well-formedness:
     For each output note j:
       a. commitment_j = PoseidonHash(output_notes[j])
       b. commitment_j matches the public commitments[j]
       c. RangeCheck(output_notes[j].value, 0, 2^64)    (no negative values)

  3. Value conservation:
     sum(input_notes[*].value) = sum(output_notes[*].value) + fee

  4. Program execution (if the transaction involves program logic):
     The BPF program executed correctly on these inputs → these outputs.
     This is either:
       a. Proven via zkVM (BPF execution trace → circuit constraints)
       b. Proven via program-specific circuit (compiled from BPF at deploy time)
       c. Proven via Tier 2 MPC (for multi-party programs — proof comes from MPC)
```

### Constraint Counts (Approximate)

| Component | Constraints | Notes |
|-----------|------------|-------|
| PoseidonHash (per call) | ~200-300 | Used for commitments, nullifiers |
| Merkle proof (depth 32) | ~10,000 | 32 × PoseidonHash |
| Range check (64-bit) | ~128 | Per value field |
| Value conservation | ~10 | Simple addition check |
| **Per input note** | **~10,500** | Commitment + Merkle + nullifier |
| **Per output note** | **~500** | Commitment + range check |
| **Base circuit (2 in, 2 out)** | **~22,000** | Without program execution |
| Program execution (simple transfer) | ~1,000-5,000 | Depends on program |
| Program execution (zkVM, 1000 inst) | ~500,000-2,000,000 | General VM proving |
| **Total (simple transfer)** | **~25,000** | Fast to prove (~5-10 seconds) |
| **Total (complex program via zkVM)** | **~2,000,000+** | Slower (~30-120 seconds) |

### Circuit for MPC Output Verification (Tier 2)

When the MPC cluster executes a multi-party program, it produces output commitments
and nullifiers. But how does the sequencer know the MPC computed correctly?

Three options (from strongest to weakest guarantees):

**Option A: MPC + ZK Proof (strongest)**

The MPC cluster jointly generates a ZK proof alongside the computation. This is
possible with MPC-in-the-head techniques or by having the MPC output feed into a
ZK circuit.

```
MPC cluster computes:
  result = match_orders(encrypted_orders)
  proof = zk_prove(result is correct)

Sequencer verifies: proof
Trust: mathematical (same as Tier 1)
Cost: MPC computation + proof generation overhead
```

**Option B: Threshold Signatures (moderate)**

The MPC cluster threshold-signs the output. If t-of-n nodes sign, the output is
accepted. This relies on the honest majority assumption.

```
MPC cluster computes:
  result = match_orders(encrypted_orders)
  signature = threshold_sign(result, cluster_keys)

Sequencer verifies: threshold signature
Trust: at least 1 of n nodes is honest (Arcium's model)
Cost: MPC computation + one signing round
```

**Option C: Optimistic + Fraud Proof (weakest but cheapest)**

Accept the MPC output, allow a challenge window. If anyone can prove the output
is wrong (by revealing their inputs and showing the computation doesn't match),
the MPC cluster is slashed.

```
MPC cluster computes:
  result = match_orders(encrypted_orders)

Sequencer: accepts result, starts challenge window (e.g., 24 hours)
Anyone: can submit fraud proof during window
Trust: at least one honest watcher
Cost: minimal (no extra computation unless challenged)
```

**Recommended**: Start with Option B (threshold signatures via Arcium's built-in
cheater identification). Upgrade to Option A for high-value computations.

---

## ZK-Friendly Design Decisions

### Hash Functions

| Hash | Constraints (ZK) | Multiplications (MPC) | Use |
|------|------------------|----------------------|-----|
| **Poseidon** | ~200-300 | ~200 | Primary: commitments, nullifiers, Merkle tree |
| **Pedersen** | ~1,500 | ~1,500 | Value commitments (homomorphic property) |
| SHA-256 | ~30,000 | ~30,000 | L1 interop only — never inside circuits |
| Keccak-256 | ~40,000 | ~40,000 | Ethereum interop only |

**Why Poseidon everywhere**: It's designed for arithmetic circuits (native field
operations). Both ZK circuits (Tier 1) and MPC protocols (Tier 2) operate over
arithmetic fields, so Poseidon is efficient in both. SHA-256 operates on bits,
which are expensive to represent in field arithmetic.

### Field Choice

ZK circuits and MPC protocols operate over a **finite field** F_p. The field must be:

- Large enough for security (~254 bits, matching BN254 or BLS12-381 curves)
- Compatible with the chosen proving system
- Compatible with the MPC secret sharing scheme

Using the same field for ZK (Tier 1) and MPC (Tier 2) means notes created in one
tier can be spent in the other — critical for cross-tier compatibility.

### Making BPF ZK-Provable

The core challenge: proving BPF VM execution in ZK without revealing the execution trace.

**The problem**: ZK proofs work over arithmetic circuits (addition and multiplication
over a finite field). A VM has registers, memory, jumps, conditionals — none of which
are natively "arithmetic."

**Three approaches** (from most general to most efficient):

#### Approach A: Full zkVM

Encode the entire BPF VM as an arithmetic circuit. Every instruction, every memory
access, every register update becomes constraints.

```
BPF: ADD64 r0, r1
Circuit: r0_next = r0_current + r1_current                (1 constraint)

BPF: LDXDW r0, [r1 + offset]
Circuit:
  addr = r1 + offset                                       (1 constraint)
  memory_consistency_check(addr, r0_next, timestamp)       (~100 constraints)
  range_check(addr, valid_region)                          (~50 constraints)

BPF: JEQ r0, r1, +5
Circuit:
  diff = r0 - r1                                           (1 constraint)
  is_zero = (diff == 0) ? 1 : 0                            (~10 constraints)
  pc_next = is_zero * (pc + 5) + (1-is_zero) * (pc + 1)   (3 constraints)
```

~10-200 constraints per BPF instruction. 10,000 instructions = millions of constraints.

**Advantage**: Any BPF program works automatically.
**Cost**: Proof generation ~30-120 seconds for complex programs.
**Projects doing this**: RISC Zero (RISC-V), SP1 (RISC-V), Valida (custom ISA).

#### Approach B: Program-Specific Circuits

Compile each BPF program into its own optimized circuit at deploy time. Removes
VM overhead (instruction decoding, program counter logic, general memory checking).

```
BPF program (token transfer):       Compiled circuit:
  load balance_A                       balance_A_new = balance_A_old - amount
  load balance_B                       balance_B_new = balance_B_old + amount
  sub balance_A, amount                RangeCheck(balance_A_new >= 0)
  add balance_B, amount                (~130 constraints total)
  store both
  (dozens of BPF instructions)
```

**Advantage**: ~100x fewer constraints. Fast proofs (~5 seconds).
**Cost**: Complex compiler. Not all BPF programs can be converted (no dynamic dispatch).

#### Approach C: Hybrid (Recommended)

- **Development**: Write programs in C, test with ZBPF VM (fast, debuggable)
- **Deployment (Tier 1)**: Compile to optimized circuit OR prove via zkVM
- **Deployment (Tier 2)**: Run as MPC-BPF on restricted subset (inherently proven by MPC)
- **Deployment (Tier 3)**: Run as standard BPF (no proving needed, transparent)

```
Developer workflow:
  1. Write program in C (targeting BPF)
  2. Test with ZBPF VM (fast, transparent, debuggable)
  3. Tag functions as private/mpc/public
  4. Private functions → compile to circuit (Approach B) or prove via zkVM (Approach A)
  5. MPC functions → verify against MPC-BPF subset, deploy to Arcium
  6. Public functions → deploy as standard BPF to rollup
  7. Differential test: circuit output == VM output for all test cases
```

The BPF VM remains the **source of truth** for program semantics. Circuits and MPC
programs are derived from it and validated against it.

---

## Linkability Analysis

### Attack Surface Map

```
On-chain data visible to an observer:

  Per transaction:
    - N nullifiers (fixed count, padded with dummies)
    - M commitments (fixed count, padded with dummies)
    - 1 proof blob (fixed size)
    - 1 Merkle root (same for many transactions in an epoch)
    - M encrypted notes (fixed size)
    - M ephemeral keys (for stealth address derivation)

  Global:
    - Commitment tree (all leaves, all historical roots)
    - Nullifier set (all spent nullifiers)
    - Public state tree (transparent)
    - Block timestamps and ordering
```

### Stealth Addresses — Detailed

**Problem**: If Alice uses the same public key for all notes, all notes sent to her
are trivially linked.

**Solution**: Every note uses a one-time **stealth address** derived via ECDH.

```
Setup (once):
  Alice generates:
    a_spend (secret), A_spend = a_spend * G  (public)
    a_view  (secret), A_view  = a_view  * G  (public)
  Alice publishes meta-address: (A_spend, A_view)

Sending (per note):
  Bob generates ephemeral key:
    r (secret), R = r * G  (public, included in transaction)
  Bob computes shared secret:
    S = r * A_view  (ECDH)
  Bob derives stealth address:
    stealth_pubkey = A_spend + Hash(S) * G
  Bob encrypts note to:
    encryption_key = KDF(S)

Receiving (scanning):
  Alice sees ephemeral key R in a new transaction.
  Alice computes:
    S' = a_view * R  (same ECDH shared secret, since a_view * R = a_view * r * G = r * A_view)
  Alice derives:
    expected_stealth = A_spend + Hash(S') * G
  If expected_stealth matches the note's owner field → it's Alice's note.
  Alice decrypts using encryption_key = KDF(S').
  Alice can spend using:
    stealth_secret = a_spend + Hash(S')
```

**Result**: Every note has a unique stealth address. An observer sees different
addresses on every transaction — no way to cluster them without `a_view`.

**Viewing key tradeoff**: Alice can share `a_view` with an auditor. The auditor can
scan and identify all of Alice's notes (read-only), but cannot spend them (needs
`a_spend`). This enables selective compliance without full transparency.

### Fixed Transaction Shape

All transactions use the same shape: **2 input nullifiers + 2 output commitments**.

```
Transfer 1→1 (send 50 to Bob):
  Nullifier 0: real (spending Alice's 100-value note)
  Nullifier 1: DUMMY (valid nullifier for a zero-value note)
  Commitment 0: real (Bob's 50-value note)
  Commitment 1: real (Alice's 50-value change note)

Transfer 2→1 (consolidate two notes):
  Nullifier 0: real (spending note A)
  Nullifier 1: real (spending note B)
  Commitment 0: real (merged note)
  Commitment 1: DUMMY (zero-value note with random salt)

An observer cannot distinguish these — both have 2 nullifiers and 2 commitments.
Dummy entries have valid proofs (they're real zero-value notes in the tree).
```

### Metadata Minimization

| Metadata | What it leaks | Mitigation |
|----------|--------------|------------|
| Transaction size | Operation type | Fixed size for all transactions |
| Proof size | Which circuit / which tier | Fixed proof size (pad smaller proofs) |
| Submission timing | Real-world activity patterns | Random delay, batching, or mixnet |
| IP address | Physical identity | Tor / relay network |
| Fee amount | Transaction type | Fixed fees per transaction |
| Encrypted note count | Number of recipients | Fixed count (pad with dummies) |
| MPC cluster membership | User identity | Rotate clusters (see mpc-prover-layer.md) |
| MPC computation time | Program complexity | Fixed-time execution (pad with dummy ops) |
| Block position | Priority, timing | Sequencer shuffles transaction order within block |

### Remaining Linkability Vectors (Honest Assessment)

Even with all mitigations, some linkability is hard to eliminate:

| Vector | Risk Level | Notes |
|--------|-----------|-------|
| Timing (tx submitted right after receiving a note) | Medium | Batching helps but doesn't eliminate |
| Note value amounts (unusual amounts like 1337.42) | Low | Range proofs don't reveal amounts, but programs should avoid leaking via public outputs |
| Deposit/withdrawal to/from L1 | High | Bridging to transparent chains is inherently linkable. Use shield pools. |
| Long-term behavioral patterns | Medium | Statistical analysis over many transactions. Harder to mitigate. |
| Viewing key compromise | High | If a_view leaks, all past and future notes are linkable |

---

## Compliance and Selective Disclosure

Privacy by default does not mean zero accountability. The system supports optional,
user-initiated disclosure:

### Viewing Keys

```
Share a_view with auditor → auditor can SEE all your notes (read-only)
                           → auditor CANNOT spend your notes (needs a_spend)
                           → auditor CANNOT see other users' notes
```

Use case: Regulatory compliance. Fund managers proving holdings to auditors.

### Proof of Source (ZK)

Generate a ZK proof that your funds came from a non-sanctioned source, without
revealing the actual source:

```
Proof: "I can show a chain of note transfers from a known-clean origin
        to my current note, and none of the intermediate steps involve
        a sanctioned address."

Verifier learns: funds are clean
Verifier does NOT learn: the actual transaction history or amounts
```

### Transaction Receipts

Prove to a counterparty that you paid them, without revealing it to anyone else:

```
Alice proves to Bob: "I created a note with commitment C, and C contains
                      value=100, asset=TOKEN_A, owner=Bob's_stealth_address"

Bob can verify this against the commitment tree.
Nobody else can verify (they don't know the note preimage).
```

### Selective Attribute Disclosure

Prove a property about your state without revealing the state itself:

```
"My balance is > 1000"  (without revealing the exact balance)
"My account is > 30 days old"  (without revealing creation date)
"I hold TOKEN_A"  (without revealing how much)
```

These are standard ZK proof constructions — the user creates a proof against their
private note data, and anyone can verify the proof.

---

## Private Transaction Lifecycle (Complete)

Putting it all together — the full lifecycle of a private transaction from user
intent to L1 finality:

```
1. USER INTENT
   Alice wants to send 50 TOKEN_A to Bob privately.

2. NOTE SELECTION (client-side)
   Alice's local note store finds a spendable note:
     Note: {owner: alice_stealth_1, value: 100, asset: TOKEN_A, salt: 0xabc...}
     Commitment: C_0 (exists at leaf index 42 in the commitment tree)

3. STEALTH ADDRESS DERIVATION (client-side)
   Alice generates ephemeral key r, computes:
     Bob's stealth address = B_spend + Hash(r * B_view) * G
     Encryption key = KDF(r * B_view)

4. NOTE CONSTRUCTION (client-side)
   Output note 0: {owner: bob_stealth, value: 50, asset: TOKEN_A, salt: random_1}
   Output note 1: {owner: alice_stealth_2, value: 50, asset: TOKEN_A, salt: random_2}
     (change note back to Alice, new stealth address)

5. BPF VM EXECUTION (client-side, Tier 1)
   Run the token transfer program locally:
     Input: Alice's note (100 TOKEN_A)
     Output: Bob's note (50) + Alice's change note (50)
     Check: value conservation, ownership, authorization
     Result: r0 = 0 (success)

6. ZK PROOF GENERATION (client-side)
   Prove all constraints:
     - Alice owns the input note (knows preimage + secret key)
     - Input note exists in the tree (Merkle proof against root R)
     - Nullifier is correct (Hash(C_0, alice_secret))
     - Output commitments are well-formed
     - Values balance: 100 = 50 + 50 + 0 (fee=0 for simplicity)
     - BPF program executed correctly
   Time: ~10-15 seconds

7. TRANSACTION SUBMISSION
   Alice submits to sequencer:
     {
       nullifiers: [Hash(C_0, alice_sk), DUMMY_NULLIFIER],
       commitments: [C_bob, C_alice_change],
       proof: <ZK proof>,
       merkle_root: R,
       encrypted_outputs: [Encrypt(bob_note, bob_key), Encrypt(change_note, alice_key)],
       ephemeral_keys: [R_bob, R_alice],
     }

8. SEQUENCER VERIFICATION
   - Verify ZK proof: valid ✓
   - Check nullifiers not in set: not found ✓
   - Check Merkle root R is recent: valid ✓
   - Append C_bob and C_alice_change to commitment tree
   - Add both nullifiers to nullifier set
   - Include in next block

9. RECIPIENT DISCOVERY
   Bob scans new encrypted notes in the block:
     For each (encrypted_note, ephemeral_key R):
       S' = b_view * R
       expected_stealth = B_spend + Hash(S') * G
       Try decrypt with KDF(S')
       → Success! Bob finds his 50 TOKEN_A note.
       Bob stores the full note locally for future spending.

10. L1 FINALITY
    Sequencer posts to L1:
      - New state root (updated commitment tree root)
      - Batch ZK proof (aggregated proof for all transactions in the block)
      - Encrypted notes (for recipient scanning)
      - Nullifier set updates
    L1 contract verifies the batch proof. State is final.
```

---

## Open Design Questions

### 1. Proving System Choice

| System | Proof Size | Verify Time | Prove Time | Trusted Setup? | Best For |
|--------|-----------|------------|-----------|----------------|----------|
| **Groth16** | ~200 bytes | ~1ms | Minutes | Yes (per-circuit) | Small fixed circuits |
| **PLONK** | ~500 bytes | ~3ms | Minutes | Universal (one-time) | Moderate circuits |
| **Halo2** | ~5KB | ~10ms | Minutes | No | Recursive proofs |
| **STARKs** | ~50-200KB | ~5ms | Seconds | No | zkVM, large circuits |

**Recommendation**: STARKs for the zkVM (proving BPF execution — large circuits,
no trusted setup, fast proving with FRI). STARK-to-SNARK wrapping for on-chain
verification (compress ~100KB STARK to ~200 byte SNARK for cheap L1 verification).

### 2. Proof Aggregation

Each transaction produces a proof. Verifying N proofs individually on L1 is expensive.

**Solution**: The sequencer aggregates all proofs in a block into a single batch proof.
One L1 verification covers the entire block (hundreds of transactions).

```
Individual proofs: [proof_1, proof_2, ..., proof_N]
Aggregated proof:  batch_proof (verifies all N in one check)
L1 cost: O(1) regardless of N
```

This is standard recursive SNARK composition (Halo2) or STARK batching.

### 3. Cross-Tier State Compatibility

Notes created in Tier 1 (client-side ZK) must be spendable in Tier 2 (MPC), and
vice versa. This requires:

- Same commitment scheme (Poseidon with same parameters)
- Same Merkle tree (shared commitment tree)
- Same nullifier derivation (same hash, same secret key format)
- Same finite field (both ZK circuits and MPC operate over the same F_p)

This is a hard constraint on the implementation — field choice must be decided early
and shared across all components.

### 4. Fee Payment

| Option | Privacy | Complexity |
|--------|---------|-----------|
| Public fee field | Fee amount visible, nothing else | Simplest |
| Fee note to sequencer | Fee hidden from public, sequencer sees it | Moderate |
| Fee in value conservation | `inputs = outputs + fee` where fee is public signal | Clean |

**Recommendation**: Fee as a public signal in the value conservation equation.
The fee amount is visible but reveals nothing about the transaction contents.

### 5. BPF-to-Circuit Pipeline

The path from C source code to a ZK-provable circuit:

```
Option A (dual compilation):
  C source → BPF bytecode (for testing in ZBPF)
  C source → Noir/Circom (for ZK proving)
  Differential test: both produce the same outputs

Option B (BPF-to-circuit transpiler):
  C source → BPF bytecode → circuit constraints
  Single source of truth, automated conversion

Option C (zkVM):
  C source → BPF bytecode → execute in zkVM → proof
  Most general, most expensive, no per-program compilation needed
```

Start with Option C (zkVM) for generality. Optimize hot programs with Option B.
Use Option A for programs where circuit performance is critical.
