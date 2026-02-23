# Implementation Plan: BPF VM Transaction Execution

Step-by-step plan to evolve ZBPF from a 3-opcode MVP into a full transaction-executing VM.

## Scope

We are building the **execution engine** — the component that runs on every validator to
process transactions. This includes:

- The BPF virtual machine (ZBPF)
- The account model and serialization layer
- The transaction runtime (verify, execute, commit/rollback)
- Syscalls for programs to interact with the host
- Cross-program invocation (CPI)

We are **not** building:

- Networking (gossip, TPU, block propagation)
- Consensus (Tower BFT, leader schedule, voting)
- Transaction routing (RPC, Gulf Stream)
- Block production or replay pipeline

Those layers sit above the execution engine. Our runtime takes a transaction and a state
store, executes it, and returns the result. A future networking/consensus layer would call
into our runtime.

## Determinism Constraint

Since every validator independently re-executes every transaction (deterministic replay),
our VM **must produce identical results given identical inputs**. This is a hard requirement
that affects every phase:

- **No system time** — programs cannot call `gettimeofday()` or similar. Time is only
  available through deterministic sysvars (Clock), which are set per-slot.
- **No randomness** — no `rand()`, no reading `/dev/urandom`. Any randomness must come
  from deterministic on-chain sources (e.g., slot hashes).
- **No floating point** — IEEE 754 has platform-dependent rounding in edge cases.
  All math must be integer-only.
- **Deterministic gas metering** — instruction counting must be exact, not approximate.
  Every validator must agree on whether a program exceeded its compute budget.
- **No undefined behavior in opcodes** — division by zero returns 0 (not a trap),
  shift amounts are masked, overflow wraps. Every edge case must have a defined result.

These constraints are checked at multiple levels: the bytecode verifier (Phase 1.8)
rejects programs that use forbidden constructs, and the syscall layer (Phase 3) only
exposes deterministic operations.

---

## Phase 1: Complete the ZBPF VM

**Goal**: Execute real compiled C programs deterministically.

Everything else depends on this — without load/store and jumps, no real program can run.

### 1.1 ALU64 Instructions (register-register and register-immediate)

Currently implemented: `MOV64_IMM (0xb7)`, `ADD64_IMM (0x07)`.

Add the remaining ALU64 operations:

```
Register-Immediate:          Register-Register:
  0x17 SUB64_IMM               0x0f ADD64_REG
  0x27 MUL64_IMM               0x1f SUB64_REG
  0x37 DIV64_IMM               0x2f MUL64_REG
  0x47 OR64_IMM                0x3f DIV64_REG
  0x57 AND64_IMM               0x4f OR64_REG
  0x67 LSH64_IMM               0x5f AND64_REG
  0x77 RSH64_IMM               0x6f LSH64_REG
  0x87 NEG64                   0x7f RSH64_REG
  0x97 MOD64_IMM               0x9f MOD64_REG
  0xa7 XOR64_IMM               0xaf XOR64_REG
  0xbf MOV64_REG               0xcf ARSH64_REG
  0xc7 ARSH64_IMM
```

**Edge cases that must be deterministic:**
- Division by zero: `dst = 0` (not a trap)
- Modulo by zero: `dst = 0`
- Shift by >= 64: mask shift amount to `imm & 63`
- Overflow: wrapping arithmetic (Rust's `wrapping_add`, `wrapping_mul`, etc.)

**Test**: Compile C programs with arithmetic, verify output matches rbpf. Include edge cases.

### 1.2 ALU32 Instructions

Same operations as ALU64 but on 32-bit values. Result is **zero-extended** to 64-bit.
Opcodes: 0x04, 0x0c, 0x14, 0x1c, 0x24, 0x2c, 0x34, 0x3c, etc.

**Test**: C programs with `uint32_t` arithmetic.

### 1.3 Jump Instructions

```
0x05  JA          (unconditional jump)
0x15  JEQ_IMM     0x1d  JEQ_REG
0x25  JGT_IMM     0x2d  JGT_REG
0x35  JGE_IMM     0x3d  JGE_REG
0x45  JNE_IMM     0x4d  JNE_REG
0x55  JSGT_IMM    0x5d  JSGT_REG
0x65  JSGE_IMM    0x6d  JSGE_REG
0xa5  JLT_IMM     0xad  JLT_REG
0xb5  JLE_IMM     0xbd  JLE_REG
0xc5  JSLT_IMM    0xcd  JSLT_REG
0xd5  JSLE_IMM    0xdd  JSLE_REG
```

Jump targets are `PC + offset + 1` (offset is relative to the next instruction).
Must bounds-check the target — jumping out of the program is an error, not UB.

**Test**: C programs with if/else, loops, switch statements.

### 1.4 Memory Load/Store Instructions

Programs need to read/write memory to do anything useful. This is where the VM
stops being a calculator and becomes a real execution environment.

```
Load:                           Store (immediate):
  0x18  LDDW (64-bit imm)        0x62  STB
  0x61  LDXW (32-bit)            0x6a  STH
  0x69  LDXH (16-bit)            0x72  STW
  0x71  LDXB (8-bit)             0x7a  STDW
  0x79  LDXDW (64-bit)
                                Store (register):
                                  0x63  STXB
                                  0x6b  STXH
                                  0x73  STXW
                                  0x7b  STXDW
```

**Memory model** — define addressable regions with bounds checking:

| Region | Address Range | Size | Purpose |
|--------|--------------|------|---------|
| Input | `0x000000000..` | Variable | Account data + instruction data (set by runtime) |
| Stack | `0x200000000..0x200000200` | 512 bytes | Call stack, r10 = top of stack |
| Heap  | `0x300000000..0x300008000` | 32KB | Dynamic allocation (optional) |

Every load/store must validate the address falls within a valid region. Out-of-bounds
access returns an error (not UB, not a segfault — a clean VM error).

`LDDW (0x18)` is special: it's the only 2-instruction-wide opcode. It loads a full
64-bit immediate by combining the `imm` fields of two consecutive instructions.

**Test**: C programs with arrays, structs, pointers.

### 1.5 Call & Internal Function Calls

```
0x85  CALL  — dispatch to syscall table (call imm)
0x95  EXIT  — return from program (already implemented)
```

For now, `call` dispatches to a `HashMap<u32, SyscallFn>` where each entry is a host
function. Start with just a stub that logs the call index.

Internal function calls (BPF subprograms) use `call rel` — the offset variant. These:
1. Push return address onto the stack
2. Push callee-saved registers (r6-r9) onto the stack
3. Jump to `PC + imm`
4. On `exit`, pop and restore

**Test**: C programs that call helper functions.

### 1.6 Byte Swap Instructions

```
0xd4  LE (host to little-endian, with imm = 16/32/64)
0xdc  BE (host to big-endian, with imm = 16/32/64)
```

### 1.7 ELF Loader

Port the ELF loading logic from rbpf/src/main.rs into zbpf so it can load compiled
`.o` files:

1. Parse ELF headers (use the `elf` crate)
2. Extract `.text` section (program bytecode)
3. Decode raw bytes into `BpfInst` structs (8 bytes per instruction)
4. Load into VM instruction vector

**Test**: `clang -target bpf -O2 -o program.o program.c` -> zbpf loads and executes it.

### 1.8 Bytecode Verifier

Before executing any program, verify it is safe:

- All jump targets are within bounds
- No unreachable instructions after unconditional jumps
- Program ends with `exit`
- No writes to r10 (read-only frame pointer)
- `LDDW` is always followed by its second instruction word
- All register reads are from previously written registers (no use of uninitialized regs)
- No division/modulo where the verifier can statically prove divisor is zero (optional)

The verifier runs **once** when a program is loaded/deployed, not on every execution.
This is cheap insurance against malformed programs crashing the VM.

**Deliverable**: ZBPF can load, verify, and execute any C program that rbpf can.
Differential testing confirms identical outputs.

---

## Phase 2: Account Model, State Store & Serialization

**Goal**: Define the account model and state store so programs can read/write account data.

The state store is needed here (not later) because the transaction runtime in Phase 4
needs somewhere to load and commit accounts.

### 2.1 Create the Runtime Crate

Create a new crate in the workspace: `runtime/` for all runtime types.

```
runtime/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── account.rs
    ├── state.rs
    └── ...
```

### 2.2 Account Struct

```rust
pub struct Account {
    pub pubkey: [u8; 32],
    pub lamports: u64,
    pub data: Vec<u8>,
    pub owner: [u8; 32],
    pub executable: bool,
    pub rent_epoch: u64,
}
```

### 2.3 State Store (In-Memory)

```rust
pub struct StateStore {
    accounts: HashMap<[u8; 32], Account>,
}

impl StateStore {
    fn get(&self, pubkey: &[u8; 32]) -> Option<&Account>;
    fn set(&mut self, pubkey: [u8; 32], account: Account);
    fn snapshot(&self) -> StateSnapshot;
    fn rollback(&mut self, snapshot: StateSnapshot);
}
```

Starts as a `HashMap`. The `snapshot()`/`rollback()` methods clone the relevant accounts
before execution and restore them on failure. This is what enables atomic transactions.

Can later be swapped for a Merkle tree (for state proofs) or persistent store
(rocksdb/sled) without changing the interface.

### 2.4 Account Serialization

Serialize accounts into the flat byte layout the VM expects (matching the layout
described in architecture.md). The program reads this through `r1`.

```rust
fn serialize_instruction_context(
    program_id: &[u8; 32],
    accounts: &[AccountMeta],
    instruction_data: &[u8],
) -> Vec<u8>
```

### 2.5 Account Deserialization

After VM execution, read back the modified account data from the same memory region:

```rust
fn deserialize_instruction_context(
    buffer: &[u8],
    accounts: &mut [Account],
)
```

### 2.6 Program-side SDK (C header)

Create a minimal C header (`programs/sdk/sol.h`) that programs include:

```c
typedef struct {
    uint8_t *pubkey;
    uint64_t *lamports;
    uint8_t *data;
    uint64_t data_len;
    uint8_t *owner;
    uint8_t is_signer;
    uint8_t is_writable;
} SolAccountInfo;

typedef struct {
    SolAccountInfo *accounts;
    uint64_t num_accounts;
    uint8_t *data;
    uint64_t data_len;
    uint8_t *program_id;
} SolParameters;

uint64_t sol_deserialize(const uint8_t *input, SolParameters *params);
```

**Test**: Write a C program that reads account data, modifies it, and returns success.
Verify the runtime picks up the changes after deserialization.

**Deliverable**: Account model, state store, and serialization round-trip working.

---

## Phase 3: Syscall Table

**Goal**: Programs can call host functions for logging, hashing, memory ops.

All syscalls must be **deterministic** — same inputs, same outputs, every time, on
every platform. This rules out anything that touches the OS, filesystem, network,
clock, or random number generator.

### 3.1 Syscall Dispatch

```rust
type SyscallFn = fn(r1: u64, r2: u64, r3: u64, r4: u64, r5: u64, vm: &mut Vm) -> u64;

struct SyscallTable {
    table: HashMap<u32, SyscallFn>,
}
```

Wire `CALL imm` in the VM to look up `imm` in the syscall table and invoke the function.
Unknown syscall index = VM error (not a no-op, not UB).

### 3.2 Core Syscalls

Implement in order of usefulness:

| Priority | Syscall | Purpose | Deterministic? |
|----------|---------|---------|----------------|
| 1 | `sol_log(msg, len)` | Debug logging | Yes (output is side-effect, not state) |
| 2 | `sol_log_64(a,b,c,d,e)` | Log 5 u64 values | Yes |
| 3 | `sol_memcpy(dst,src,n)` | Memory copy | Yes |
| 4 | `sol_memset(dst,val,n)` | Memory fill | Yes |
| 5 | `sol_memcmp(a,b,n,result)` | Memory compare | Yes |
| 6 | `sol_sha256(vals,num,output)` | SHA-256 | Yes |
| 7 | `sol_keccak256(vals,num,output)` | Keccak-256 | Yes |

**Not implemented (non-deterministic):**
- `time()`, `clock_gettime()` — use Clock sysvar instead
- `rand()` — use slot hashes or VRF instead
- Any floating-point math

Each syscall must validate its memory arguments (pointers within valid VM regions,
no overlapping src/dst for memcpy, etc.) and deduct compute units.

**Test**: C programs that call each syscall. Verify log output, hash values, etc.

**Deliverable**: Programs can log, hash, and manipulate memory through syscalls.

---

## Phase 4: Transaction Runtime

**Goal**: Execute full transactions with signature verification and atomic commit/rollback.

This is where it all comes together. The runtime is the "main loop" that a validator
calls for every transaction in a block.

### 4.1 Transaction & Message Structs

```rust
pub struct Transaction {
    pub signatures: Vec<[u8; 64]>,
    pub message: Message,
}

pub struct Message {
    pub account_keys: Vec<[u8; 32]>,
    pub recent_blockhash: [u8; 32],
    pub instructions: Vec<CompiledInstruction>,
}

pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub account_indices: Vec<u8>,
    pub data: Vec<u8>,
}
```

### 4.2 Signature Verification

Use the `ed25519-dalek` crate:

```rust
fn verify_transaction(tx: &Transaction) -> Result<(), Error> {
    // For each signature + corresponding account_key:
    //   verify ed25519 signature over serialized message
    // Mark which accounts are signers
}
```

This runs **before** any VM execution. Failed sig verify = transaction dropped entirely
(not even charged fees in our simplified model).

### 4.3 Built-in Programs

Seed the state store with native programs that are implemented in Rust, not BPF:

| Program | Pubkey | Purpose |
|---------|--------|---------|
| System Program | `1111...1111` | Create accounts, transfer lamports, assign ownership |
| BPF Loader | `BPFLoader...` | Deploy and upgrade BPF programs |

The System Program handles:
- `CreateAccount` — allocate a new account with space and lamports
- `Transfer` — move lamports between accounts
- `Assign` — change account owner

These are privileged — they bypass the BPF VM and execute as native Rust. The runtime
checks `program_id` and dispatches to the native handler if it matches a built-in,
otherwise loads the BPF bytecode and runs the VM.

### 4.4 Runtime Execution Loop

```rust
fn execute_transaction(
    tx: &Transaction,
    state: &mut StateStore,
) -> Result<(), Error> {
    // 1. Verify all signatures
    verify_signatures(&tx)?;

    // 2. Load all accounts from state store
    let mut accounts = load_accounts(state, &tx.message.account_keys)?;

    // 3. Snapshot for rollback
    let snapshot = accounts.clone();

    // 4. Execute each instruction sequentially
    for ix in &tx.message.instructions {
        let program_id = &tx.message.account_keys[ix.program_id_index as usize];

        // Check if this is a built-in program
        if is_builtin(program_id) {
            execute_builtin(program_id, &mut accounts, &ix)?;
            continue;
        }

        // Load BPF program bytecode
        let program_account = state.get(program_id)?;
        let bytecode = &program_account.data;

        // Gather accounts for this instruction
        let ix_accounts = gather_accounts(&accounts, &ix.account_indices, &tx);

        // Serialize into VM memory
        let input = serialize_instruction_context(program_id, &ix_accounts, &ix.data);

        // Create and run VM
        let mut vm = Vm::new(bytecode, input, &syscall_table);
        let result = vm.run()?;

        if result != 0 {
            // Rollback ALL changes from this transaction
            accounts = snapshot;
            return Err(Error::ProgramError(result));
        }

        // Deserialize changes back
        deserialize_instruction_context(vm.memory(), &mut accounts);

        // Verify ownership and access rules
        verify_account_changes(&snapshot, &accounts, program_id)?;
    }

    // 5. Commit: write all accounts back to state store
    for account in &accounts {
        state.set(account.pubkey, account.clone());
    }

    Ok(())
}
```

### 4.5 Ownership & Access Checks

After each instruction, the runtime verifies the program didn't violate rules:

- Only the owning program modified an account's `data`
- Only the owning program debited an account's `lamports`
- Anyone can credit an account's `lamports` (add, not subtract)
- No program modified accounts not marked as `is_writable`
- Account data didn't grow beyond its allocated size
- `executable` accounts were not modified
- `owner` was not changed (only System Program can do this)

If any check fails, the entire transaction rolls back.

### 4.6 Compute Budget

Each transaction gets a compute budget (e.g., 200,000 units). The VM decrements this
as it executes instructions. Syscalls deduct additional units based on their cost
(e.g., SHA-256 costs more than a log). If the budget hits zero, the transaction fails
and rolls back.

This must be **deterministic** — every validator must agree on the exact compute cost.

**Test**: Build transactions with transfer instructions. Verify balances change
atomically. Test rollback by having the second instruction fail. Test compute budget
exhaustion.

**Deliverable**: Full transaction execution with sig verify, atomic commit/rollback,
ownership checks, and compute metering.

---

## Phase 5: Cross-Program Invocation (CPI)

**Goal**: Programs can call other programs.

This is the most complex feature. It enables composability — a DEX program calling a
token program, an escrow program calling the system program, etc.

### 5.1 sol_invoke_signed Syscall

When program A calls `sol_invoke_signed(instruction, accounts, signers_seeds)`:

1. Runtime suspends VM A
2. Validates the instruction and accounts
3. If `signers_seeds` provided, derive PDA and add as signer
4. Serializes accounts for program B
5. Executes program B in a new (or reused) VM
6. If program B fails, propagate failure back (program A fails too)
7. Propagates account changes back to program A's memory region
8. Resumes program A

### 5.2 PDA (Program Derived Addresses)

```rust
fn create_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Result<[u8; 32], Error> {
    // SHA-256(seeds || program_id || "ProgramDerivedAddress")
    // Must NOT be on the ed25519 curve (ensures no private key exists)
}

fn find_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> ([u8; 32], u8) {
    // Try bump seeds 255..0 until create_program_address succeeds
}
```

PDAs allow programs to "sign" for accounts they own without a private key. This is how
programs can hold funds, own token accounts, etc.

### 5.3 CPI Depth Limit

Limit CPI call depth to 4 (same as Solana) to prevent stack overflow and excessive
compute usage. Depth is tracked in the runtime and passed to each VM invocation.

### 5.4 Account Re-borrowing

When program A does CPI to program B, both see the same accounts. Changes program B
makes must be visible to program A when it resumes. This means the runtime must:

1. Serialize program A's current account state for program B
2. After program B finishes, deserialize changes back into program A's memory
3. Update program A's view of account data in-place

**Test**: Program A calls Program B which modifies an account. Verify the change is
visible to Program A after the CPI returns. Test CPI depth limit. Test PDA signing.

**Deliverable**: Full CPI support with PDA signing and account re-borrowing.

---

## Phase 6: Testing & Validation

### 6.1 Unit Tests

- Every opcode has a test (including edge cases: div by zero, overflow, shift limits)
- Serialization round-trips
- Syscall behavior (correct hashes, bounds checking)
- Account ownership enforcement
- Bytecode verifier catches bad programs

### 6.2 Integration Tests

- End-to-end transaction execution
- Multi-instruction transactions (atomic commit)
- Rollback on failure (second instruction fails, first reverts)
- CPI chains (A -> B -> C, up to depth 4)
- Compute budget exhaustion
- Signature verification (valid + invalid)

### 6.3 Differential Testing

Run the same C program on both rbpf and zbpf, compare outputs for every test case.
This is the strongest validation that our VM is correct — if both produce the same
result for thousands of inputs, we can be confident.

### 6.4 Determinism Testing

Run the same transaction on two independent instances of our runtime. Verify the
resulting state is byte-identical. This validates the determinism constraint —
if two "validators" disagree, we have a consensus bug.

### 6.5 Example Programs

Build a set of example programs to validate the full stack:

1. **Hello world** — basic program that logs and returns success
2. **Counter** — program that increments a counter stored in account data
3. **Transfer** — lamport transfer via System Program
4. **Token** — simplified SPL-token-like program (mint, transfer, burn)
5. **Escrow** — two-party atomic swap using CPI and PDAs

---

## Workspace Structure (Final)

```
bpf-vm/
├── Cargo.toml              # workspace
├── docs/
│   ├── architecture.md
│   └── implementation-plan.md
├── programs/               # example C programs
│   ├── program.c
│   ├── counter.c
│   ├── token.c
│   ├── escrow.c
│   └── sdk/
│       └── sol.h           # C SDK header
├── rbpf/                   # reference implementation (external crate)
│   ├── Cargo.toml
│   └── src/main.rs
├── zbpf/                   # our custom VM
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── vm.rs           # VM core (registers, memory, execution loop)
│       ├── opcodes.rs      # opcode dispatch
│       ├── memory.rs       # memory regions and bounds checking
│       ├── verifier.rs     # bytecode verifier
│       ├── elf_loader.rs   # ELF parser and program loader
│       └── syscalls.rs     # syscall implementations
├── runtime/                # transaction runtime
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── account.rs      # Account struct + serialization
│       ├── transaction.rs  # Transaction structs
│       ├── executor.rs     # execution loop (verify, run, commit/rollback)
│       ├── state.rs        # StateStore (HashMap, snapshot/rollback)
│       ├── compute.rs      # compute budget tracking
│       ├── builtins/
│       │   ├── mod.rs
│       │   └── system.rs   # System Program (native)
│       └── cpi.rs          # cross-program invocation
└── tests/                  # integration tests
    ├── vm_tests.rs
    ├── transaction_tests.rs
    ├── determinism_tests.rs
    └── cpi_tests.rs
```

---

## Dependencies (New)

| Crate | Purpose | Phase |
|-------|---------|-------|
| `elf` | ELF parsing (already used in rbpf) | 1 |
| `ed25519-dalek` | Signature verification | 4 |
| `sha2` | SHA-256 for syscalls + PDAs | 3 |
| `tiny-keccak` | Keccak-256 for syscalls | 3 |
| `curve25519-dalek` | PDA: check if point is on ed25519 curve | 5 |
| `sled` (optional) | Persistent state store | 2 (later) |

---

## Phase Dependencies

```
Phase 1: Complete VM
    |
    +---> Phase 2: Account Model + State Store
              |
              +---> Phase 3: Syscalls
              |         |
              |         +---> Phase 4: Transaction Runtime
              |                   |
              |                   +---> Phase 5: CPI
              |                            |
              +----------------------------+---> Phase 6: Testing (ongoing)
```

Each phase is independently testable and builds on the previous one. Testing (Phase 6)
runs in parallel from Phase 1 onward — every phase should have tests before moving on.
