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

## eBPF Opcode Reference

### Instruction Classes

| Class | Value | Description |
|-------|-------|-------------|
| LD    | 0x00  | Non-standard load operations |
| LDX   | 0x01  | Load into register operations |
| ST    | 0x02  | Store from immediate operations |
| STX   | 0x03  | Store from register operations |
| ALU   | 0x04  | 32-bit arithmetic operations |
| JMP   | 0x05  | 64-bit jump operations |
| JMP32 | 0x06  | 32-bit jump operations |
| ALU64 | 0x07  | 64-bit arithmetic operations |

### ALU64 Instructions (64-bit)

| Opcode | Mnemonic   | Pseudocode        |
|--------|------------|-------------------|
| 0x07   | add        | dst += imm        |
| 0x0f   | add        | dst += src        |
| 0x17   | sub        | dst -= imm        |
| 0x1f   | sub        | dst -= src        |
| 0x27   | mul        | dst *= imm        |
| 0x2f   | mul        | dst *= src        |
| 0x37   | div        | dst = (src!=0) ? (dst/imm) : 0 |
| 0x3f   | div        | dst = (src!=0) ? (dst/src) : 0 |
| 0x47   | or         | dst \|= imm       |
| 0x4f   | or         | dst \|= src       |
| 0x57   | and        | dst &= imm        |
| 0x5f   | and        | dst &= src        |
| 0x67   | lsh        | dst <<= imm       |
| 0x6f   | lsh        | dst <<= src       |
| 0x77   | rsh        | dst >>= imm       |
| 0x7f   | rsh        | dst >>= src       |
| 0x87   | neg        | dst = -dst        |
| 0x97   | mod        | dst %= imm        |
| 0x9f   | mod        | dst %= src        |
| 0xa7   | xor        | dst ^= imm        |
| 0xaf   | xor        | dst ^= src        |
| 0xb7   | mov        | dst = imm         |
| 0xbf   | mov        | dst = src         |
| 0xc7   | arsh       | dst >>= imm (sign) |
| 0xcf   | arsh       | dst >>= src (sign) |

### ALU Instructions (32-bit)

Same as ALU64 but operates on 32-bit values (result zero-extended to 64-bit).

| Opcode | Mnemonic   | Pseudocode        |
|--------|------------|-------------------|
| 0x04   | add        | dst += imm        |
| 0x0c   | add        | dst += src        |
| 0x14   | sub        | dst -= imm        |
| 0x1c   | sub        | dst -= src        |
| 0x24   | mul        | dst *= imm        |
| 0x2c   | mul        | dst *= src        |
| 0x34   | div        | dst = (src!=0) ? (dst/imm) : 0 |
| 0x3c   | div        | dst = (src!=0) ? (dst/src) : 0 |
| 0x44   | or         | dst \|= imm       |
| 0x4c   | or         | dst \|= src       |
| 0x54   | and        | dst &= imm        |
| 0x5c   | and        | dst &= src        |
| 0x64   | lsh        | dst <<= imm       |
| 0x6c   | lsh        | dst <<= src       |
| 0x74   | rsh        | dst >>= imm       |
| 0x7c   | rsh        | dst >>= src       |
| 0x84   | neg        | dst = -dst        |
| 0x94   | mod        | dst %= imm        |
| 0x9c   | mod        | dst %= src        |
| 0xa4   | xor        | dst ^= imm        |
| 0xac   | xor        | dst ^= src        |
| 0xb4   | mov        | dst = imm         |
| 0xbc   | mov        | dst = src         |
| 0xc4   | arsh       | dst >>= imm (sign) |
| 0xcc   | arsh       | dst >>= src (sign) |

### Jump Instructions

| Opcode | Mnemonic   | Pseudocode                    |
|--------|------------|-------------------------------|
| 0x05   | ja         | PC += offset                  |
| 0x15   | jeq        | PC += offset if dst == imm    |
| 0x1d   | jeq        | PC += offset if dst == src   |
| 0x25   | jgt        | PC += offset if dst > imm    |
| 0x2d   | jgt        | PC += offset if dst > src    |
| 0x35   | jge        | PC += offset if dst >= imm   |
| 0x3d   | jge        | PC += offset if dst >= src   |
| 0xa5   | jlt        | PC += offset if dst < imm    |
| 0xad   | jlt        | PC += offset if dst < src    |
| 0xb5   | jle        | PC += offset if dst <= imm   |
| 0xbd   | jle        | PC += offset if dst <= src   |
| 0x45   | jne        | PC += offset if dst != imm   |
| 0x4d   | jne        | PC += offset if dst != src  |
| 0x55   | jsgt       | PC += offset if dst > imm (signed) |
| 0x5d   | jsgt       | PC += offset if dst > src (signed) |
| 0x65   | jsge       | PC += offset if dst >= imm (signed) |
| 0x6d   | jsge       | PC += offset if dst >= src (signed) |
| 0xc5   | jslt       | PC += offset if dst < imm (signed) |
| 0xcd   | jslt       | PC += offset if dst < src (signed) |
| 0xd5   | jsle       | PC += offset if dst <= imm (signed) |
| 0xdd   | jsle       | PC += offset if dst <= src (signed) |

### Jump Helper Instructions

| Opcode | Mnemonic   | Description                    |
|--------|------------|--------------------------------|
| 0x85   | call       | Call helper function           |
| 0x95   | exit       | Return from program            |

### Load/Store Instructions

| Opcode | Mnemonic   | Pseudocode                         |
|--------|------------|-----------------------------------|
| 0x18   | lddw       | dst = imm64 (2 instructions)     |
| 0x58   | ldabsw     | Load word (legacy packet access)  |
| 0x50   | ldxw       | dst = *(u32 *)(src + offset)     |
| 0x61   | ldxh       | dst = *(u16 *)(src + offset)     |
| 0x71   | ldxb       | dst = *(u8 *)(src + offset)      |
| 0x79   | ldxdw      | dst = *(u64 *)(src + offset)     |
| 0x62   | stb        | *(u8 *)(dst + offset) = imm      |
| 0x6a   | sth        | *(u16 *)(dst + offset) = imm     |
| 0x72   | stw        | *(u32 *)(dst + offset) = imm     |
| 0x7a   | stdw       | *(u64 *)(dst + offset) = imm     |
| 0x63   | stxb       | *(u8 *)(dst + offset) = src      |
| 0x6b   | stxh       | *(u16 *)(dst + offset) = src     |
| 0x73   | stxw       | *(u32 *)(dst + offset) = src     |
| 0x7b   | stxdw      | *(u64 *)(dst + offset) = src    |

### Byte Swap Instructions

| Opcode | Mnemonic   | Description                    |
|--------|------------|--------------------------------|
| 0xd4   | le16       | dst = htole16(dst)            |
| 0xdc   | le32       | htole32(dst)                  |
| 0xd4   | le64       | htole64(dst)                  |
| 0xd4   | be16       | dst = htobe16(dst)            |
| 0xdc   | be32       | htobe32(dst)                  |
| 0xd4   | be64       | htobe64(dst)                  |

### Registers

eBPF has 10 general-purpose 64-bit registers:

| Register | Purpose                          |
|----------|----------------------------------|
| r0       | Return value from functions, exit value |
| r1-r5    | Arguments for function calls   |
| r6-r9    | Callee-saved registers          |
| r10       | Read-only frame pointer         |

## References

- [rbpf crate](https://docs.rs/rbpf/0.4.1)
- [elf crate](https://docs.rs/elf/0.8.0)
- [LLVM BPF Target](https://clang.llvm.org/docs/UsersManual.html#cmdoption-ftarget-bpf)
- [eBPF instruction set](https://github.com/iovisor/bpf-docs/blob/master/eBPF.md)
