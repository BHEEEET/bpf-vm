#[derive(Clone, Copy)]
struct BpfInst {
    opcode: u8,  // operation code
    dst: u8,     // destination register
    src: u8,     // source register
    offset: i16, // jump offset (if any)
    imm: i32,    // immediate value
}

const REGISTER_COUNT: usize = 11;
const STACK_SIZE: usize = 512;

struct Vm {
    regs: [u64; REGISTER_COUNT], // r0–r10
    stack: [u8; STACK_SIZE],     // simple stack
    pc: usize,                   // program counter
    instructions: Vec<BpfInst>,  // program instructions
    instruction_limit: u64,      // max allowed instructions
    instruction_count: u64,      // how many executed
}

impl Vm {
    fn run(&mut self) -> Result<u64, String> {
        while self.pc < self.instructions.len() {
            if self.instruction_count >= self.instruction_limit {
                return Err("Out of gas".to_string());
            }

            let insn = self.instructions[self.pc];
            self.instruction_count += 1;

            match insn.opcode {
                0xb7 => {
                    // MOV64_IMM: dst = imm
                    self.regs[insn.dst as usize] = insn.imm as u64;
                }

                0x07 => {
                    // ADD64_IMM: dst += imm
                    self.regs[insn.dst as usize] += insn.imm as u64;
                }

                0x95 => {
                    // EXIT
                    return Ok(self.regs[0]);
                }

                _ => return Err(format!("Unsupported opcode: {}", insn.opcode)),
            }

            self.pc += 1;
        }

        Err("No exit instruction found".to_string())
    }
}

fn main() -> Result<(), String> {
    // Example program:
    // r0 = 10
    // r0 += 32
    // exit

    let program = vec![
        BpfInst {
            opcode: 0xb7,
            dst: 0,
            src: 0,
            offset: 0,
            imm: 10,
        }, // r0 = 10
        BpfInst {
            opcode: 0x07,
            dst: 0,
            src: 0,
            offset: 0,
            imm: 32,
        }, // r0 += 32
        BpfInst {
            opcode: 0x95,
            dst: 0,
            src: 0,
            offset: 0,
            imm: 0,
        }, // exit
    ];

    let mut vm = Vm {
        regs: [0; REGISTER_COUNT],
        stack: [0; STACK_SIZE],
        pc: 0,
        instructions: program,
        instruction_limit: 100,
        instruction_count: 0,
    };

    let result = vm.run()?;
    println!("Program returned: {}", result);

    Ok(())
}
