# Private Multisig — Implementation Design

A 2-of-3 threshold multisig for the ZK privacy rollup. No transaction details, signer identities, or approval counts are visible to observers.

---

## Goals

| Goal | Description |
|------|-------------|
| **Privacy** | No linkability between signers, transactions, or approvals |
| **Threshold** | 2-of-3 scheme — any 2 signers can execute |
| **No Intent Leakage** | Observers cannot determine what transaction was approved |
| **No Signer Linkability** | Observers cannot determine who signed or how many |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  WORKFLOW: 2-of-3 Threshold Multisig                          │
│                                                                 │
│  ┌──────────┐                                                  │
│  │ Proposer  │  1. Creates transaction                          │
│  │ (Signer)  │  2. Shares with other signers (encrypted)       │
│  └────┬─────┘                                                  │
│       │                                                          │
│       │ "I want to send X to Y"                                 │
│       v                                                          │
│  ┌─────────┐   ┌─────────┐   ┌─────────┐                        │
│  │ Signer  │   │ Signer  │   │ Signer  │  3. Each signer      │
│  │         │◄──┤         │◄──┤         │     submits approval  │
│  └────┬────┘   └────┬────┘   └────┬────┘     (encrypted)       │
│       │              │              │                            │
│       └──────────────┼──────────────┘                            │
│                      │                                            │
│                      v                                            │
│              ┌───────────────┐                                    │
│              │ MPC CLUSTER   │  4. Threshold check                │
│              │ (Arcium)     │     - Verify M approvals           │
│              │               │     - Generate threshold sig      │
│              │               │     - No signer visibility        │
│              └───────┬───────┘                                    │
│                      │                                            │
│                      v                                            │
│              ┌───────────────┐                                    │
│              │  SEQUENCER    │  5. Verify threshold sig         │
│              │               │     Commit to rollup              │
│              └───────────────┘                                    │
└─────────────────────────────────────────────────────────────────┘
```

---

## Design Decisions

| Decision | Value | Rationale |
|----------|-------|-----------|
| **Threshold** | 2-of-3 | MVP — enough for testing, simple to reason about |
| **Signers see tx** | Yes | Proposer creates tx, shares with signers |
| **MPC Provider** | Arcium | Proven infrastructure, threshold signatures built-in |
| **Signature** | Schnorr threshold | Doesn't reveal who signed |
| **Execution** | Immediate | No delay for MVP |
| **Chain** | Rollup (Tier 3) | Multisig lives on the privacy rollup |
| **Asset** | Native lamports | MVP — no token program needed |

---

## Key Generation (One-Time Setup)

### Participants
Three signers: Alice (index 0), Bob (index 1), Carol (index 2)

### Process

```
1. Each signer generates random secret: s_i
2. Run MPC (via Arcium) to compute:
   - Threshold public key P = s_0 + s_1 + s_2 * G
   - Verification keys for each signer: V_i = s_i * G
   - Signing shares for each participant
3. Each signer i receives: {s_i, V_i}
4. Public (on-chain): {P, M=2, N=3, [V_0, V_1, V_2]}
```

### On-Chain State

```rust
struct MultisigAccount {
    // Threshold parameters
    threshold: u8,                    // M = 2
    num_signers: u8,                 // N = 3
    
    // Public keys
    threshold_pk: [u8; 32],          // Threshold public key
    verification_keys: [[u8; 32; 3]], // Individual verification keys
    
    // State
    nonce: u64,                      // Incremented per transaction
    is_initialized: bool,
}
```

---

## Transaction Flow

### 1. Proposer Creates Transaction

```rust
struct MultisigTransaction {
    // Visible to all signers (encrypted payload)
    proposer_index: u8,               // Who created this tx
    nonce: u64,                      // Must match account nonce
    
    // Transaction details (encrypted for MPC)
    recipient: [u8; 32],             // Recipient's stealth address
    amount: u64,                     // Lamports to send
    
    // Metadata
    encrypted_payload: Vec<u8>,       // Encrypted transaction data
    payload_commitment: [u8; 32],   // Poseidon hash of encrypted payload
}
```

The proposer:
1. Creates the transaction with recipient and amount
2. Encrypts the payload (for privacy from MPC nodes)
3. Computes `payload_commitment = Poseidon(encrypted_payload)`
4. Submits to MPC cluster or shares directly with signers

### 2. Signers Submit Approvals

Each signer (excluding proposer or including — either works):

```rust
struct ApprovalShare {
    signer_index: u8,                // 0, 1, or 2
    tx_commitment: [u8; 32],         // Hash of transaction being approved
    
    // Partial Schnorr signature
    // R_i = k_i * G (k_i random per approval)
    // s_i = k_i + H(R_i, msg) * s_i (secret share)
    R: [u8; 32],                    
    s_share: [u8; 32],              
}
```

Each signer:
1. Receives transaction (from proposer or MPC)
2. Verifies the transaction is acceptable
3. Generates partial signature using their signing share
4. Submits approval to MPC cluster (encrypted)

### 3. MPC Threshold Execution

The MPC cluster runs the threshold signing program:

```
Input:
  - encrypted_transaction: Transaction payload
  - approvals: Vec<ApprovalShare> from signers
  - threshold_pk: Public key
  - M: required signatures (2)

Process:
  1. Decrypt transaction (MPC nodes hold decryption shares)
  2. For each approval:
     a. Verify partial signature is valid for signer
     b. Check signer_index is authorized
  3. Check: approvals.len() >= M (threshold met)
  4. Aggregate partial signatures into threshold signature:
     R = sum(R_i)
     s = sum(s_i) mod n
  5. Output: ThresholdSignature { R, s }

Output:
  - threshold_signature: (R, s)
  - transaction_commitment: for data availability
  - nullifiers: for the notes being spent
  - new_commitments: for recipient + change notes
```

**Critical**: The threshold signature `(R, s)` does NOT reveal:
- Who signed (could be any 2 of 3)
- How many signed (just that threshold was met)
- The transaction details

### 4. Sequencer Verification

The sequencer receives:

```rust
struct SignedTransaction {
    // Threshold signature (verifies M-of-N approved)
    threshold_sig: ThresholdSignature,
    
    // Transaction commitment (for data availability)
    tx_commitment: [u8; 32],
    
    // Privacy outputs
    nullifiers: [[u8; 32]; 2],       // Spent notes
    commitments: [[u8; 32]; 2],     // New notes (recipient + change)
    
    // Encrypted transaction (for DA)
    encrypted_tx: Vec<u8>,
    
    // Proof
    proof: Vec<u8>,                  // MPC proof or threshold sig
}
```

Sequencer verifies:
1. Threshold signature is valid for `threshold_pk`
2. Nullifiers not previously spent
3. Merkle root is valid
4. Adds commitments to tree, nullifiers to set

### 5. Recipient Discovery

Recipient scans for notes encrypted to their stealth address:
- Uses their viewing key to derive shared secret
- Decrypts the note
- Note contains the lamports sent

---

## Program Execution (How It Runs Privately)

The multisig is a **multi-party operation**, so it runs entirely in **Tier 2 (MPC)**. No single party (not even the signers) sees the full picture.

### Execution Flow

```
┌────────────────────────────────────────────────────────────────────────┐
│  EXECUTION: Private Multisig on the Rollup                           │
│                                                                        │
│  Step 1: Transaction Intent (Client-side, Tier 1)                    │
│  ┌──────────────┐                                                     │
│  │  Proposer    │  Builds: {recipient, amount, nonce}               │
│  │              │  Encrypts for MPC cluster                          │
│  │              │  → encrypted_tx_blob                               │
│  └──────┬───────┘                                                     │
│         │                                                              │
│         ▼                                                              │
│  Step 2: Approvals (Client-side, Tier 1)                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │  Signer A    │  │  Signer B    │  │  Signer C    │              │
│  │              │  │              │  │              │              │
│  │  Approve ✓   │  │  Approve ✓   │  │  (not needed)│              │
│  │  → share_A   │  │  → share_B   │  │              │              │
│  └──────┬───────┘  └──────┬───────┘  └──────────────┘              │
│         └────────────┬────┘                                           │
│                      │                                                │
│                      ▼                                                │
│  Step 3: MPC Execution (Tier 2)                                        │
│  ┌─────────────────────────────────────────────────────────────────┐  │
│  │  MPC CLUSTER (Arx Nodes 1-4)                                    │  │
│  │                                                                  │  │
│  │  Input:                                                         │  │
│  │    - encrypted_tx_blob                                           │  │
│  │    - approval_shares from signers                               │  │
│  │    - threshold_pk                                                │  │
│  │                                                                  │  │
│  │  What runs in the VM:                                           │  │
│  │    - BPF program: threshold_sign()                              │  │
│  │    - Verifies each approval is valid                            │  │
│  │    - Checks: approvals.len() >= 2                              │  │
│  │    - Aggregates partial sigs → threshold_signature             │  │
│  │    - Emits nullifiers + commitments                            │  │
│  │                                                                  │  │
│  │  Output:                                                        │  │
│  │    - threshold_signature (R, s)                                 │  │
│  │    - nullifiers[2]                                              │  │
│  │    - commitments[2]                                             │  │
│  │    - proof                                                       │  │
│  └─────────────────────────────────────────────────────────────────┘  │
│                      │                                                │
│                      ▼                                                │
│  Step 4: Sequencer Verification (Tier 3)                            │
│  ┌─────────────────────────────────────────────────────────────────┐  │
│  │  SEQUENCER                                                      │  │
│  │                                                                  │  │
│  │  Input:                                                         │  │
│  │    - threshold_signature                                        │  │
│  │    - nullifiers                                                 │  │
│  │    - commitments                                                │  │
│  │    - proof                                                       │  │
│  │                                                                  │  │
│  │  Verification (NO re-execution):                               │  │
│  │    1. Verify threshold_signature against threshold_pk           │  │
│  │    2. Check nullifiers not spent                               │  │
│  │    3. Verify Merkle proof for commitments                     │  │
│  │    4. Add to state                                             │  │
│  │                                                                  │  │
│  │  What the sequencer SEES:                                       │  │
│  │    - Only opaque cryptographic blobs                           │  │
│  │    - NOT: who signed, what tx said, how many signed           │  │
│  └─────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────┘
```

### The BPF Program That Runs in MPC

The MPC-BPF VM executes this program on secret-shared data:

```c
// threshold_sign.bpf - runs in MPC cluster

uint64_t threshold_sign(
    ApprovalShare *approvals,    // From signers
    uint8_t num_approvals,
    ThresholdPK *pk,
    uint8_t threshold_M,
    
    // Outputs
    ThresholdSignature *out_sig,
    Nullifier *out_nullifiers,
    Commitment *out_commitments
) {
    // 1. Verify each approval
    for (uint8_t i = 0; i < num_approvals; i++) {
        if (!verify_approval(&approvals[i], pk)) {
            return ERROR_INVALID_APPROVAL;
        }
    }
    
    // 2. Check threshold
    if (num_approvals < threshold_M) {
        return ERROR_THRESHOLD_NOT_MET;  // Need 2, got 1
    }
    
    // 3. Aggregate signatures (Schnorr)
    // R = sum(R_i), s = sum(s_i)
    aggregate_schnorr(approvals, num_approvals, out_sig);
    
    // 4. Emit nullifiers for spent notes
    for (uint8_t i = 0; i < num_approvals; i++) {
        out_nullifiers[i] = compute_nullifier(approvals[i].note_commitment);
    }
    
    // 5. Emit new commitments (recipient + change)
    // (transaction details decrypted inside MPC)
    Commitment recipient_note = create_note_commitment(
        recipient_stealth,
        amount,
        new_salt
    );
    Commitment change_note = create_note_commitment(
        signer_stealth, 
        change_amount,
        new_salt_2
    );
    out_commitments[0] = recipient_note;
    out_commitments[1] = change_note;
    
    return SUCCESS;
}
```

This BPF program runs in **MPC mode** — every operation happens on secret-shared data. No single MPC node sees the full execution.

### How the MPC-BPF VM Works

The VM differs between tiers:

```rust
// Standard VM (Tier 1/3) - plaintext execution
pub struct Vm {
    regs: [u64; 11],      // Plaintext values
    memory: Vec<u8>,
    // ...
}

// MPC VM (Tier 2) - secret-shared execution
pub struct MpcVm {
    regs: [FieldElement; 11],  // Secret-shared values
    // Each node has shares: 
    // Node 1: reg[0].share[0], reg[0].share[1], etc.
    // ...
}
```

Operations in MPC mode:

```rust
// BPF: ADD r0, r1
// In MPC mode:
r0_share = r0_share + r1_share  // LOCAL, no communication!

// BPF: MUL r0, r1  
// In MPC mode:
r0_share = beaver_triple_mul(r0_share, r1_share)  // 1 round-trip between nodes
```

### What Each Party Sees

| Party | Sees | Privacy |
|-------|------|---------|
| **Signer** | Their own approval, transaction details they created | ✓ Own data only |
| **Other signers** | Nothing — encrypted | ✓ No linkability |
| **MPC nodes** | Only their share of each value | ✓ Can't reconstruct |
| **Sequencer** | threshold_sig, nullifiers, commitments, proof | ✓ Nothing useful |
| **Observers** | Opaque blobs, fixed transaction shape | ✓ No info |

### Verification vs Re-Execution

**Solana model**: Every validator re-executes the transaction
```
tx → validator1 executes → validator2 executes → ... → consensus
```

**Our model**: Sequencer verifies proof, no re-execution
```
tx → MPC executes → outputs proof → sequencer verifies proof → commit
```

The sequencer never runs the BPF program. It just checks:
1. Threshold signature is valid (mathematical check)
2. Nullifiers not spent (set membership)
3. Commitments valid (Merkle proof)

This is **O(1)** verification cost vs **O(n)** re-execution.

### Sequencer Integration

```rust
// rollup-executor/src/execute_multisig.rs

pub fn execute_private_multisig(
    rollup_state: &mut RollupState,
    multisig_tx: &MultisigTransaction,
) -> Result<()> {
    
    // 1. Get the multisig account
    let multisig = rollup_state.get_multisig(&multisig_tx.multisig_pubkey)?;
    
    // 2. Receive from MPC cluster (Tier 2 output)
    let mpc_output = multisig_tx.mpc_output;
    
    // 3. Verify threshold signature (NO re-execution!)
    let valid = verify_threshold_sig(
        &mpc_output.threshold_sig,
        &multisig.threshold_pk,
        &mpc_output.message_hash,
    )?;
    if !valid {
        return Err(Error::InvalidThresholdSignature);
    }
    
    // 4. Check nullifiers not spent
    for nullifier in &mpc_output.nullifiers {
        if rollup_state.nullifier_set.contains(nullifier) {
            return Err(Error::DoubleSpend);
        }
    }
    
    // 5. Verify commitments against Merkle root
    for commitment in &mpc_output.commitments {
        if !rollup_state.commitment_tree.verify(commitment) {
            return Err(Error::InvalidCommitment);
        }
    }
    
    // 6. Commit to state
    for nullifier in &mpc_output.nullifiers {
        rollup_state.nullifier_set.insert(*nullifier);
    }
    for commitment in &mpc_output.commitments {
        rollup_state.commitment_tree.insert(*commitment);
    }
    
    // 7. Update multisig nonce
    multisig.nonce += 1;
    
    Ok(())
}
```

### Summary

The multisig executes entirely in **Tier 2 (MPC)**:
1. Signers submit encrypted approvals to MPC cluster
2. MPC-BPF VM runs the threshold signing program on secret-shared data
3. Each MPC node only sees shares — no single node knows what happened
4. Output: threshold signature + nullifiers + commitments
5. Sequencer verifies mathematically — never sees the program or data

This keeps it private: no one (not signers, not MPC nodes, not sequencer) sees the full picture.

---

## Client-Side vs MPC Execution: When Is Each Faster?

For the multisig smart contract, we have two execution options. Here's the performance comparison:

| Metric | Client-Side (Tier 1) | MPC Layer (Tier 2) |
|--------|---------------------|-------------------|
| **VM Execution** | ~microseconds | ~milliseconds |
| **Proof Generation** | ~5-60 seconds | N/A (inherently proven) |
| **End-to-End** | ~5-60 seconds | ~1-5 seconds |
| **Privacy** | Signers see each other's data | No one sees full data |
| **Signer Linkability** | High | None |

### When Client-Side Would Be Faster

Client-side is faster when:
- The program is very small (< 500 BPF instructions)
- You don't need multi-party computation
- You can tolerate signers seeing each other's data

### When MPC Would Be Faster

MPC is faster when:
- The program involves multiple users' data
- The program is simple (threshold check, signature aggregation)
- You need privacy between signers

For the multisig, **MPC is the right choice** because:
1. Multiple signers need to combine approvals without seeing each other
2. Threshold signature aggregation is simple (~100-200 instructions)
3. Privacy is a hard requirement — client-side would leak signer identities

### The Crossover Point

The multisig threshold signing program is ~200 instructions — well below the crossover point (~500-2000), making MPC faster.

### Decision: Why We Choose MPC for Multisig

| Factor | Client-Side | MPC | Winner |
|--------|-------------|-----|--------|
| **Speed** | 10-60 sec | 1-5 sec | MPC |
| **Privacy** | Signers see each other | No one sees full data | MPC |
| **Signer linkability** | High | None | MPC |
| **Implementation** | Simpler | More complex | Client-side |

**For the multisig, MPC wins on both speed and privacy.**

### Hybrid Approach (Future Optimization)

For complex smart contracts, we can use a hybrid:
1. Client-side (Tier 1): Each signer validates their own approval locally (~2000 instructions per signer, parallel)
2. MPC Layer (Tier 2): Combine proofs, check threshold, aggregate sigs (~200 instructions, very fast in MPC)
3. Sequencer (Tier 3): Verify threshold sig, commit to state

---

## Privacy Analysis

| What | Who Sees | Leakage |
|------|----------|---------|
| Transaction amount | MPC nodes (on shares) | None externally |
| Transaction recipient | MPC nodes (on shares) | None externally |
| Who signed | Nobody | None |
| How many signed (exactly) | Nobody | None |
| Link proposer → transaction | Encrypted commitment | None |
| Link signers to each other | Nobody | None |

### Linkability Mitigations

| Vector | Mitigation |
|--------|------------|
| Transaction size | Fixed size — pad all txs to same length |
| Approval timing | Random delays or batching |
| Signing pattern | Different signer combinations don't link |
| Amount correlation | Different amounts → different commitments (not linkable) |

---

## Component Breakdown

```
multisig/
├── contracts/
│   └── multisig.rs              # On-rollup multisig program
│
├── programs/
│   └── threshold_bpf/          # BPF programs for Tier 2 MPC
│       ├── lib.rs               # Main threshold logic
│       └── syscalls.rs          # Custom syscalls
│
├── sdk/
│   ├── rust/
│   │   ├── src/
│   │   │   ├── client.rs        # MultisigClient
│   │   │   ├── transaction.rs   # Transaction types
│   │   │   ├── approval.rs      # Approval shares
│   │   │   └── crypto.rs        # Threshold crypto
│   │   └── Cargo.toml
│   │
│   └── typescript/
│       └── index.ts             # JS/TS SDK
│
├── mpc/
│   └── threshold-sign/          # Arcium MPC program
│       ├── src/
│       │   └── lib.rs
│       └── Cargo.toml
│
└── tests/
    ├── integration.rs           # Full flow tests
    ├── threshold.rs            # Threshold signature tests
    └── security.rs             # Malicious actor tests
```

---

## Implementation Phases

### Phase 1: Foundation (Week 1)

| Task | Description |
|------|-------------|
| Threshold keygen | MPC-based key generation for 2-of-3 |
| On-chain multisig account | Store PK, verification keys, nonce |
| Basic SDK types | Transaction, Approval structures |

### Phase 2: Transaction Flow (Week 2)

| Task | Description |
|------|-------------|
| Proposer transaction creation | Build and encrypt tx |
| Approval submission | Signers generate partial sigs |
| MPC threshold program | Arcium program for threshold signing |

### Phase 3: Integration (Week 3)

| Task | Description |
|------|-------------|
| Arcium integration | Connect to MPC cluster |
| Sequencer verification | Verify threshold sigs |
| End-to-end test | Full happy path |

### Phase 4: Privacy Hardening (Week 4)

| Task | Description |
|------|-------------|
| Fixed transaction sizes | Pad all transactions |
| Timing randomization | Prevent timing analysis |
| Integration tests | Test malicious scenarios |

---

## What Already Exists vs What to Build

### Already in Repo

- ZBPF VM (basic opcodes)
- Architecture documentation
- Three-tier execution design
- Syscall framework
- Note/commitment state model

### Need to Build

- Threshold key generation
- Client SDK (Rust + TypeScript)
- On-rollup multisig contract
- Arcium MPC program
- Integration tests

---

## Open Questions

1. **Arcium access**: Do you have access to Arcium testnet? If not, we can mock the MPC layer for development.

2. **Proposer visibility**: Should the proposer be known (signer index visible) or should we hide even that?

3. **Fallback for testing**: Should we implement a simpler "all 3 sign" multisig first, then upgrade to threshold?

4. **Integration with existing rollup**: Once built, should the multisig deploy to local devnet, testnet, or both?

---

## References

- Architecture: `docs/architecture.md`
- Privacy details: `docs/privacy-architecture.md`
- MPC layer: `docs/mpc-prover-layer.md`
- State model: `docs/state-and-composability.md`
- Implementation plan: `docs/implementation-plan.md`
