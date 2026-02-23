# BPF VM Architecture

## Part 1: How Solana Does It (Background)

Understanding Solana's transparent execution model first, because our architecture
deliberately departs from it to achieve privacy.

### Transaction Routing: How a Transaction Reaches the VM

Before the VM ever runs, the transaction has to get to the right place.

```
You (wallet/dApp)
       |
       v
 RPC Node (any validator with JSON-RPC enabled)
       |
       |  The RPC knows the leader schedule — which validator
       |  produces blocks for which slots. It forwards your
       |  transaction directly to the current leader's TPU port.
       |
       +---> Current Leader (TPU port, via UDP/QUIC)
       +---> Next 1-2 Leaders (hedging in case slot changes)
```

**Key points about routing:**

- **RPC nodes are just validators** (or lightweight nodes) that expose a JSON-RPC API.
  There is nothing special about them beyond having that endpoint enabled.
- **The fast path**: RPC forwards directly to the leader's **TPU (Transaction Processing
  Unit)** port. This is what normally happens.
- **The slow path**: If you sent a transaction to a non-leader validator, it would still
  forward it to the leader through the network — it just adds latency (extra hops).
- **You don't even need an RPC** — anyone can open a direct UDP/QUIC connection to the
  leader's TPU port. MEV/trading bots do this to shave off milliseconds.
- **Gulf Stream** — Solana's name for this forwarding protocol. Validators forward
  transactions to upcoming leaders ahead of time, so there is no traditional mempool
  sitting around waiting.
- **If the leader misses your transaction** (slot ended, leader crashed), the next leader
  can pick it up. The `recent_blockhash` field gives a transaction a ~60 second window
  to land in a block.

### Who Runs the VM on Solana?

**Every validator runs the VM.** Not just the leader.

```
 Leader (block producer)
    |
    |  1. Receives transactions
    |  2. Orders them into a block
    |  3. Executes each transaction in the VM
    |  4. Streams the block + results to the cluster
    |
    +-------> Validator A: re-executes every transaction independently
    +-------> Validator B: re-executes every transaction independently
    +-------> Validator C: re-executes every transaction independently
    +-------> ...every other validator does the same
```

This is **deterministic replay**. Every validator takes the same block (same transactions
in the same order) and runs them through the same VM. Because the VM is fully deterministic
— same inputs always produce the same outputs — every validator arrives at the **identical
state**.

The leader's execution is **not trusted**. If a leader produces a block with wrong results,
validators will reject it because their own re-execution won't match.

This is why the BPF VM **forbids non-determinism**: no system time access, no randomness,
no floating point. The only inputs to the VM are the serialized accounts + instruction data,
which are identical for every validator processing that block.

### Solana's Execution Pipeline (On Each Validator)

Once a transaction lands in a block and a validator processes it:

```
 Transaction arrives (from block)
       |
       v
 1. Signature Verification (ed25519)
       |
       v
 2. Account Locking
    - All accounts referenced by the tx are locked
    - Prevents concurrent modification
       |
       v
 3. Account State Snapshot
    - Runtime saves original account state
    - This is the rollback point
       |
       v
 4. BPF VM Execution
    - Program runs inside the VM
    - It DIRECTLY MUTATES account data buffers in memory
    - Writes to account `data`, `lamports`, etc. through pointers
    - Accounts are passed via r1 (instruction context pointer)
       |
       v
 5. Commit or Rollback
    - r0 == 0 (success) -> changes are COMMITTED
    - r0 != 0 (error)   -> changes are ROLLED BACK to snapshot
       |
       v
 6. Account Unlocking
```

The VM is **not** a simulator. The program genuinely writes to account memory during
execution. The atomicity guarantee comes from commit/rollback at the end, not from
deferring writes. Think of it like a database transaction: writes happen in-place, but
aren't visible to other transactions until commit.

### What Signature Verification Actually Does

Sig verify happens **before** VM execution. It proves that the transaction was authorized
by the private key holder. The runtime marks which accounts are "signers" and passes that
information into the VM. The program can then check `account.is_signer` to enforce
authorization logic.

Sig verify does NOT directly gate state changes — the program's own logic decides what
requires a signature.

### What's Wrong With This Model

Everything is visible. On a transparent chain, any observer can see:

- **WHO** sent the transaction (signer pubkey)
- **WHO** received funds (destination account)
- **HOW MUCH** was transferred (lamports / token amounts)
- **WHAT** program was called (program_id)
- **WHAT** the instruction data was (the full intent)
- **WHEN** it happened (slot/timestamp)
- **HOW** accounts are related (shared owners, same program)

An observer can reconstruct your financial history, trading strategies, social graph,
and behavioral patterns. Intent leakage enables front-running and sandwiching.

---

## Part 2: Our Architecture — Privacy-First BPF Execution

We replace Solana's "everyone re-executes everything transparently" model with a
**three-tier hybrid execution model** where the VM runs in different places depending
on the privacy requirements, and verification replaces re-execution.

### Core Design Principle

```
Solana:   execute everywhere, verify by re-execution, everything visible
Ours:     execute once (privately), verify by proof, nothing visible
```

Instead of every validator running the VM on plaintext data, execution happens
**privately** (client-side or in an MPC cluster), and validators only **verify a
proof** that the execution was correct. They never see the inputs, the program
logic, or the intermediate state.

### Three-Tier Execution Model

The BPF VM serves different roles depending on the privacy requirements:

```
+------------------------------------------------------------------+
|  TIER 1: CLIENT-SIDE EXECUTION (Private, ZK-proven)              |
|                                                                   |
|  WHERE: User's device (browser, wallet, CLI)                     |
|  WHAT:  Single-user operations where the user knows their data   |
|  HOW:   Run BPF VM locally -> generate ZK proof -> submit proof  |
|                                                                   |
|  Examples:                                                        |
|  - Private transfers (user knows their own amounts)              |
|  - Private state mutations (user knows their own data)           |
|  - Note creation/spending (user's notes, user's secrets)         |
|                                                                   |
|  Privacy: Hidden from everyone except the user                   |
|  Speed:   VM execution = microseconds, proof gen = 5-60 seconds  |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
|  TIER 2: MPC PROVER LAYER (Multi-party private compute)          |
|                                                                   |
|  WHERE: Decentralized MPC cluster (Arcium-style Arx nodes)      |
|  WHAT:  Multi-party operations where no single party should see  |
|         anyone else's data                                        |
|  HOW:   Secret-share inputs across nodes -> nodes jointly        |
|         execute MPC-BPF on shares -> output commitments + proof  |
|                                                                   |
|  Examples:                                                        |
|  - Order matching (sealed bids from multiple users)              |
|  - Private auctions (nobody sees losing bids)                    |
|  - Cross-user state reads (check conditions without revealing)   |
|  - Private liquidation checks (health factor stays hidden)       |
|                                                                   |
|  Privacy: Hidden from EVERYONE — no single node sees plaintext   |
|  Speed:   Seconds to minutes (MPC overhead per operation)        |
+------------------------------------------------------------------+

+------------------------------------------------------------------+
|  TIER 3: ROLLUP EXECUTION (Public, transparent)                  |
|                                                                   |
|  WHERE: Sequencer / rollup validators                            |
|  WHAT:  Public state that doesn't need privacy                   |
|  HOW:   Standard BPF VM execution (same as Solana model)         |
|                                                                   |
|  Examples:                                                        |
|  - Oracle price updates                                          |
|  - Governance vote tallying                                      |
|  - Public auction resolution                                     |
|  - Aggregate statistics (total volume, TVL)                      |
|                                                                   |
|  Privacy: None — fully transparent, everyone verifies            |
|  Speed:   Microseconds (standard VM execution)                   |
+------------------------------------------------------------------+
```

**Programs can span all three tiers.** A private DEX program might have:
- `create_order()` — Tier 1 (user creates encrypted order, client-side ZK)
- `match_orders()` — Tier 2 (MPC cluster matches orders from multiple users)
- `update_oracle_price()` — Tier 3 (public, no privacy needed)

### System Overview

```
  User A                     User B                     User C
    |                          |                          |
    | Tier 1: run VM locally   | Tier 1: run VM locally   |
    | generate ZK proof        | generate ZK proof        |
    |                          |                          |
    +--- private tx (proof) ---+--- private tx (proof) ---+
    |                          |                          |
    | encrypted order shares   | encrypted order shares   |
    v                          v                          v
+------------------------------------------------------------------+
|  TIER 2: MPC PROVER LAYER                                        |
|                                                                   |
|  Arx Node 1     Arx Node 2     Arx Node 3     Arx Node 4       |
|  [share_A1]     [share_A2]     [share_A3]     [share_A4]        |
|  [share_B1]     [share_B2]     [share_B3]     [share_B4]        |
|  [share_C1]     [share_C2]     [share_C3]     [share_C4]        |
|                                                                   |
|  Jointly execute MPC-BPF: match_orders()                         |
|  No node sees any plaintext order                                |
|  Output: commitments + nullifiers + proof                        |
+-------------------------------+----------------------------------+
                                |
                                v
+------------------------------------------------------------------+
|  SEQUENCER / ROLLUP                                              |
|                                                                   |
|  Receives from Tier 1: private txs (nullifiers, commitments,     |
|                         ZK proofs)                                |
|  Receives from Tier 2: MPC outputs (commitments, proofs)         |
|  Runs Tier 3:          public BPF VM execution                   |
|                                                                   |
|  For ALL tiers:                                                   |
|  1. Verify proofs (ZK or MPC — pure math, no re-execution)      |
|  2. Check nullifiers not spent (no double-spend)                 |
|  3. Check Merkle root is valid (state consistency)               |
|  4. Append commitments to commitment tree                        |
|  5. Record nullifiers                                            |
|  6. Execute public functions (Tier 3 only)                       |
|  7. Post state root + proofs to L1                               |
+------------------------------------------------------------------+
                                |
                                v
+------------------------------------------------------------------+
|  L1 (DATA AVAILABILITY)                                          |
|                                                                   |
|  - Rollup state root (commitment tree root)                      |
|  - Batch validity proof (aggregated ZK proof for N transactions) |
|  - Nullifier updates                                             |
|  - Encrypted notes (for recipient scanning)                      |
|  - Public state diffs (Tier 3)                                   |
+------------------------------------------------------------------+
```

### Who Runs the VM Now?

The answer is different depending on the tier:

| | Solana | Our Tier 1 | Our Tier 2 | Our Tier 3 |
|---|---|---|---|---|
| **Who executes** | Every validator | User's device | MPC cluster (Arx nodes) | Sequencer |
| **Who verifies** | Every validator (re-execution) | Sequencer (proof check) | Sequencer (proof check) | Validators (re-execution or proof) |
| **What's visible** | Everything | Proof + ciphertext | Proof + ciphertext | Everything |
| **VM runs on** | Plaintext accounts | Plaintext (local) | Secret shares | Plaintext accounts |

**Critical shift**: Validators **no longer re-execute transactions**. They verify proofs.
This is cheaper (proof verification is O(1) vs re-execution is O(n)) and reveals nothing.

---

## Part 3: State Model

### Dual State — Notes (Private) + Accounts (Public)

We maintain two parallel state structures. Private state uses a note model (UTXO-like)
for unlinkability. Public state uses a traditional account model for shared/readable data.

#### Private State: Notes

A **note** is an encrypted, one-time-use value container. It is never stored in plaintext.

```
Note (exists only in the owner's local storage):
    owner: PublicKey
    value: u64
    asset_type: [u8; 32]
    salt: [u8; 32]           random nonce (makes each note unique)
    app_data: Vec<u8>        arbitrary program state

On-chain (public):
    commitment = PoseidonHash(owner, value, asset_type, salt, app_data)
```

Nobody can reverse the commitment to learn the contents. Only the owner (who knows all
the fields) can prove ownership and spend the note.

**Why notes instead of accounts for private state?** Accounts have permanent pubkeys —
every transaction that touches an account is trivially linked. Notes are one-time-use
with unique commitments, breaking the linkability chain.

#### Commitments and Nullifiers

```
Creating a note:
    commitment = PoseidonHash(owner, value, asset_type, salt, app_data)
    → Appended to the Commitment Merkle Tree (public, but contents hidden)

Spending a note:
    nullifier = PoseidonHash(commitment, owner_secret_key)
    → Published on-chain. Checked against the Nullifier Set (no duplicates).
    → Nobody can link the nullifier back to the commitment
      (different hash domain, requires owner's secret key to connect them)
```

```
                    Commitment Tree              Nullifier Set
                    (append-only Merkle)         (duplicate check)
                          |                            |
  Create note: add leaf   |                            |
                          |                            |
  Spend note:             |                   add nullifier
                          |                            |
  Linkable?    NO — can't tell which          NO — can't reverse
               commitment matches              to find commitment
```

#### Public State: Accounts

For data that doesn't need privacy (oracle prices, governance, aggregate stats), we
use a traditional account model:

```rust
struct PublicAccount {
    pubkey: [u8; 32],
    data: Vec<u8>,
    owner: [u8; 32],
}
```

Public accounts live in a **Public State Tree** (separate from the commitment tree).
They're readable by private functions (via Merkle proof) and writable by public
functions (Tier 3).

#### State Structures

```
+--------------------------------------------+
|  Commitment Tree (Sparse Merkle, depth 32) |
|  ~4 billion leaves, Poseidon hash          |
|  Append-only: new note → new leaf          |
|  Used for: note existence proofs           |
+--------------------------------------------+

+--------------------------------------------+
|  Nullifier Set (HashSet or Sparse Merkle)  |
|  Checked on every transaction              |
|  Used for: double-spend prevention         |
+--------------------------------------------+

+--------------------------------------------+
|  Public State Tree                         |
|  Account model for public data             |
|  Readable by private functions (via proof) |
|  Writable by Tier 3 (public execution)     |
+--------------------------------------------+
```

#### ZK-Friendly Hashing

All in-circuit hashing uses **Poseidon** (~200-300 constraints per hash) instead of
SHA-256 (~30,000 constraints). Poseidon is designed for arithmetic circuits — native
field operations, minimal constraints.

---

## Part 4: The VM (ZBPF)

The BPF VM is a single codebase that runs in all three tiers, with tier-specific
adaptations.

### VM Core (Shared Across All Tiers)

```
VM {
    regs: [u64; 11]          // r0-r10 (or [Share; 11] in Tier 2)
    stack: [u8; 512]         // call stack
    memory: [u8; N]          // addressable memory (heap + input region)
    pc: usize                // program counter
    instructions: Vec<Inst>  // loaded program
    instruction_limit: u64   // compute budget (gas)
    syscall_table: HashMap<u32, fn>  // helper function dispatch
}
```

**Key difference from Solana sBPF**: We use standard BPF `call` semantics. Solana sBPF
modified the call instruction to support both internal function calls and syscalls through
a custom relocation scheme. We keep it simple — `call imm` dispatches to the syscall table.

### VM in Tier 1 (Client-Side)

Standard plaintext execution. The user runs the same VM locally. After execution, the
VM trace is fed into a ZK prover to generate a proof.

```
User's device:
  1. Load BPF program
  2. Load private inputs (notes, secrets, Merkle proofs)
  3. Execute in ZBPF (plaintext, fast)
  4. VM outputs: nullifiers, new commitments
  5. Feed execution trace into ZK prover → proof
  6. Submit: {nullifiers, commitments, proof, encrypted_notes}
```

### VM in Tier 2 (MPC Prover Layer)

The VM operates on **secret-shared** values instead of plaintext. Each Arx node runs
the VM on its own shares. Nodes communicate during multiplications and comparisons.

```
Standard ZBPF:                    MPC-ZBPF:

regs: [u64; 11]                   regs: [Share; 11]

ADD r0, r1:                        ADD r0, r1:
  r0 = r0 + r1                      r0_share += r1_share
  (one op)                           (local, no communication — FREE)

MUL r0, r1:                        MUL r0, r1:
  r0 = r0 * r1                      beaver_triple_mul(r0, r1, nodes)
  (one op)                           (1 round trip — ~1-5ms)

JEQ r0, r1, +5:                    JEQ r0, r1, +5:
  if r0 == r1 { pc += 5 }           bit = secure_compare(r0, r1)
  (branch)                           execute BOTH branches
                                     result = mux(bit, true_path, false_path)
                                     (~10-50ms)
```

Not all BPF programs can run in MPC efficiently. A **restricted BPF subset** is
enforced at deploy time for Tier 2 programs:

| | Tier 1 & 3 (Full BPF) | Tier 2 (MPC-BPF Subset) |
|---|---|---|
| ADD, SUB, MOV | Yes | Yes (free — local on shares) |
| MUL | Yes | Yes (~1-5ms per multiply) |
| Comparison/Jumps | Yes | Yes, but expensive (~10-50ms) |
| Dynamic memory access | Yes | No (must be fixed-offset) |
| Unbounded loops | Yes | No (bounds must be compile-time known) |
| Division | Yes | No (too expensive in MPC) |
| SHA-256 | Yes | No (use Poseidon instead) |

### VM in Tier 3 (Public Rollup)

Standard plaintext execution, identical to Solana's model. The sequencer runs the VM
on public accounts. No privacy, no proofs — just transparent execution and state updates.

### Syscalls

The `call imm` instruction dispatches to host-provided functions. Different tiers
provide different syscall implementations:

```
+--------+------------------------------+-------+-------+-------+
| Index  | Name                         | Tier1 | Tier2 | Tier3 |
+--------+------------------------------+-------+-------+-------+
| 0      | sol_log                      |  Yes  | No*   |  Yes  |
| 1      | sol_log_64                   |  Yes  | No*   |  Yes  |
| 2      | sol_poseidon_hash            |  Yes  |  Yes  |  Yes  |
| 3      | sol_keccak256                |  Yes  |  No   |  Yes  |
| 4      | sol_memcpy                   |  Yes  |  Yes  |  Yes  |
| 5      | sol_memset                   |  Yes  |  Yes  |  Yes  |
| 6      | sol_invoke_signed            |  Yes  |  No** |  Yes  |
| 7      | sol_create_program_address   |  Yes  |  Yes  |  Yes  |
| 8      | sol_verify_merkle_proof      |  Yes  |  Yes  |  No   |
| 9      | sol_emit_nullifier           |  Yes  |  Yes  |  No   |
| 10     | sol_emit_commitment          |  Yes  |  Yes  |  No   |
| 11     | sol_get_clock                |  No   |  No   |  Yes  |
+--------+------------------------------+-------+-------+-------+

* Logging in Tier 2 would leak information through the log contents.
  Disabled by default. Debug mode only.
** CPI in Tier 2 is complex — a program calling another program while
   both are running on secret shares. Deferred to later phase.
```

**New privacy-specific syscalls:**
- `sol_poseidon_hash` — ZK-friendly hashing (replaces SHA-256 for private state)
- `sol_verify_merkle_proof` — verify a note exists in the commitment tree
- `sol_emit_nullifier` — declare a note as spent
- `sol_emit_commitment` — create a new note commitment

---

## Part 5: Transaction Types

### Private Transaction (Tier 1 / Tier 2)

```rust
struct PrivateTransaction {
    // PUBLIC — visible on-chain
    nullifiers: [[u8; 32]; NUM_INPUTS],    // fixed count, padded with zeros
    commitments: [[u8; 32]; NUM_OUTPUTS],  // fixed count, padded with zeros
    proof: ZkProof,                         // fixed size (ZK or MPC proof)
    merkle_root: [u8; 32],                 // which state root was used

    // ENCRYPTED — only recipients can decrypt
    encrypted_outputs: [EncryptedNote; NUM_OUTPUTS],
    ephemeral_keys: [[u8; 32]; NUM_OUTPUTS],  // for stealth address derivation
}
```

**Fixed sizes everywhere**: NUM_INPUTS = 2, NUM_OUTPUTS = 2 (for example). Every
transaction looks identical in shape and size. Dummy notes pad unused slots.

The sequencer:
1. Verifies the proof (pure math — no re-execution)
2. Checks nullifiers haven't been spent
3. Checks Merkle root is recent/valid
4. Appends commitments to the commitment tree
5. Records nullifiers
6. Posts to L1

**What the sequencer sees**: Opaque 32-byte values and a proof blob. Cannot determine
who sent it, who receives, how much, or what program ran.

### Public Transaction (Tier 3)

```rust
struct PublicTransaction {
    signatures: Vec<[u8; 64]>,
    message: PublicMessage,
}

struct PublicMessage {
    account_keys: Vec<[u8; 32]>,
    recent_blockhash: [u8; 32],
    instructions: Vec<CompiledInstruction>,
}

struct CompiledInstruction {
    program_id_index: u8,
    account_indices: Vec<u8>,
    data: Vec<u8>,
}
```

Identical to Solana's model. Used for public state operations. Fully transparent.

### Hybrid Transaction

A single user action can produce **both** a private transaction and a public
transaction. Example: a private swap that updates a public price oracle.

```
User action: swap 100 TOKEN_A for TOKEN_B privately

Produces:
  PrivateTransaction:
    - Nullifies user's TOKEN_A note
    - Creates new TOKEN_B note for user
    - Creates change TOKEN_A note (if any)
    - ZK proof that amounts balance

  PublicTransaction (emitted by the program):
    - Updates public price oracle with latest trade price
    - Updates aggregate volume counter
    - No individual trade details leaked
```

---

## Part 6: Instruction Context

### Tier 1 & 3: Serialized Account Data (via r1)

When the runtime invokes a program (Tier 1 locally, Tier 3 on sequencer), it
serializes the instruction context into VM memory and sets `r1` to point to it.

```
Serialized layout in VM memory (what r1 points to):

+--------------------------------------------------+
| num_accounts: u64                                |
+--------------------------------------------------+
| For each account (Tier 3) or note (Tier 1):      |
|   is_signer: u8                                  |
|   is_writable: u8                                |
|   pubkey: [u8; 32]                               |
|   owner: [u8; 32]                                |
|   lamports: u64                                  |
|   data_len: u64                                  |
|   data: [u8; data_len]                           |
|   executable: u8                                 |
+--------------------------------------------------+
| instruction_data_len: u64                        |
| instruction_data: [u8; instruction_data_len]     |
+--------------------------------------------------+
| program_id: [u8; 32]                             |
+--------------------------------------------------+
```

After execution, the runtime deserializes this region. In Tier 1, the modified state
is converted into output commitments. In Tier 3, it's written back to the public
state tree.

### Tier 2: Secret-Shared Inputs

In Tier 2, each Arx node receives secret shares of the inputs. The serialization
layout is the same, but every value is a share, not plaintext. The MPC-ZBPF engine
operates on these shares without any node seeing the reconstructed values.

---

## Part 7: Linkability Defenses

### Stealth Addresses

Alice publishes a **meta-address** `(A_spend, A_view)`. When Bob sends her a note,
he derives a one-time stealth address using Diffie-Hellman:

```
Bob generates: ephemeral keypair (r, R = r*G)
Stealth address: S = A_spend + Hash(r * A_view) * G
Note encrypted to: key derived from DH shared secret
```

Every note Alice receives has a **different** address. No two notes are linkable
without Alice's viewing key.

### Fixed Transaction Shape

Every private transaction has exactly N nullifiers and M commitments, padded with
dummy zero-value notes. All transactions look identical in structure and size.

### Timing and Metadata

| Metadata | Leaks | Mitigation |
|----------|-------|------------|
| Transaction size | Operation type | Fixed size for all transactions |
| Submission timing | Activity patterns | Random delay or batching |
| IP address | Physical identity | Tor / mixnet / relay network |
| Fee amount | Transaction type | Fixed fees |
| Encrypted note count | Number of recipients | Fixed count (pad with dummies) |
| MPC cluster choice | User identity | Rotate clusters per computation |
| MPC computation time | Program complexity | Fixed-time execution (pad) |

---

## Part 8: Native BPF vs Solana sBPF

| Aspect | Solana sBPF | Our Native BPF |
|--------|-------------|----------------|
| **call semantics** | Modified: custom relocation for internal calls + syscalls | Standard: `call imm` dispatches to syscall table |
| **ELF format** | Custom loader with sBPF-specific relocations | Standard BPF ELF, parsed with standard tools |
| **Memory regions** | Strict: input, stack, heap, program text | Same logical separation, we define our own regions |
| **Compute budget** | ~200K compute units with per-opcode costs | `instruction_limit` — same concept, simpler accounting |
| **Bytecode verifier** | sBPF verifier | Standard verifier + MPC subset verifier for Tier 2 |
| **Toolchain** | `cargo build-sbpf` (custom) | Standard `clang -target bpf` |
| **Execution model** | Transparent re-execution on every validator | Private execution + proof verification |
| **State model** | Accounts (transparent, linkable) | Notes (private) + Accounts (public) |

### Why Native BPF?

1. **Simpler toolchain** — standard clang/llvm, no custom cargo plugin
2. **Standard ecosystem** — bpftool, llvm-objdump, etc. work out of the box
3. **ZK-compatible** — standard BPF instruction set maps cleanly to arithmetic circuits
4. **MPC-compatible** — subset of BPF maps to efficient MPC operations
5. **Same security model** — BPF sandboxing guarantees preserved

---

## Part 9: Document Map

This architecture document is the overview. Detailed designs are in separate documents:

| Document | Contents |
|----------|----------|
| `architecture.md` | This document — system overview, tiers, state model, VM design |
| `implementation-plan.md` | Phase-by-phase build plan with code examples and test criteria |
| `privacy-architecture.md` | Deep dive on ZK proving, note model, threat model, circuit design |
| `mpc-prover-layer.md` | Deep dive on MPC execution, Arcium integration, operation costs, hybrid workflows |
| `state-and-composability.md` | State freshness, note model vs accounts, CPI in privacy, flash loans, MEV resistance |
