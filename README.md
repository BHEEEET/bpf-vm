# BPF VM

A BPF (Berkeley Packet Filter) virtual machine playground with multiple implementations.

## Overview

This project contains two BPF VM implementations:

- **rbpf** - Uses the [rbpf](https://github.com/qmonnet/rbpf) crate (v0.4.1)
- **zbpf** - Custom bpf implementation

## Prerequisites

Install clang with BPF target support:

```bash
# Ubuntu/Debian
sudo apt install clang llvm
```

## Build & Run

### 1. Compile a BPF Program

Write your BPF program in C:

```c
// programs/program.c
int entry() {
    return 42;
}
```

Compile to BPF object file:

```bash
clang -target bpf -O2 -o program.o programs/program.c
```

Copy to `.so` (required by the VM):

```bash
cp program.o program.so
```

View the disassembly:

```bash
llvm-objdump -d program.o
```

### 2. Run the VM

```bash
cargo run --bin rbpf
```

Expected output:
```
Program returned: 42
```

## Project Structure

```
bpf-vm/
├── Cargo.toml          # Workspace config
├── README.md
├── programs/
│   └── program.c       # Sample BPF program
├── rbpf/               # rbpf implementation
│   ├── Cargo.toml
│   └── src/main.rs
└── zbpf/               # zbpf implementation
    ├── Cargo.toml
    └── src/main.rs
```

## Key Learnings

### rbpf API Notes

The `rbpf` crate (v0.4.1) has some quirks:

1. **No built-in ELF module** - Use the separate [`elf`](https://crates.io/crates/elf) crate (v0.8.0) to parse ELF files

2. **No `vm` module** - VM types like `EbpfVmNoData` are at the crate root, not in a submodule:
   ```rust
   use rbpf::EbpfVmNoData;
   ```

3. **Section names vary** - BPF compilers may use different section names (`.text`, `.classifier`, etc.). Check with:
   ```bash
   llvm-objdump -h program.o
   ```

4. **`Executable::from_elf`** - This API belongs to `solana_rbpf` (a fork), not the original `rbpf` crate

## Troubleshooting

### "No .classifier section found"

The code section has a different name. Check available sections:

```bash
llvm-objdump -h program.o
```

Then update the section name in `rbpf/src/main.rs` to match.

### "No .text section found"

Same issue - update the section name to match your BPF object's section.

## References

- [rbpf crate](https://docs.rs/rbpf/0.4.1)
- [elf crate](https://docs.rs/elf/0.8.0)
- [LLVM BPF Target](https://clang.llvm.org/docs/UsersManual.html#cmdoption-ftarget-bpf)
- [eBPF instruction set](https://github.com/iovisor/bpf-docs/blob/master/eBPF.md)
